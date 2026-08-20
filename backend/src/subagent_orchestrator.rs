use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::subagent::api::{InvocationMode, TokenUsage, TraceContext};
use crate::subagent::lifecycle::{ExecutionOutcome, ExecutionPhase as ReservationState};
use crate::subagent::protocol;
use crate::subagent::rules::{
    self, RoleAccess, RolePolicy, RuleActor, RuleContext, RuleEffect, RuleSet, ToolClass,
};
use crate::subagent::telemetry::{
    self, ExecutionStatus, SubagentTraceEvent, TraceEventKind, TraceRecorder,
};

pub(crate) const CONTRACT_PREFIX: &str = "CODEY_DELEGATION_V2=";
const LEGACY_CONTRACT_PREFIX_V1: &str = "CODEY_DELEGATION_V1=";
pub(crate) const POST_TOOL_HOOK_MATCHER: &str = "*";

const LEDGER_SCHEMA_VERSION: u32 = 8;
const MIN_LEDGER_SCHEMA_VERSION: u32 = 1;
const LEDGER_FILE: &str = "orchestrator-ledger-v1.json";
const LEDGER_LOCK_FILE: &str = "orchestrator-ledger-v1.lock";
const READ_ONLY_CONCURRENCY_LIMIT: usize = 3;
const WRITE_OR_MIXED_CONCURRENCY_LIMIT: usize = 2;
const DUPLICATE_TASK_ID_ERROR_CODE: &str = "CODEY_SUBAGENT_DUPLICATE_TASK_ID";
const FOLLOWUP_REQUIRES_ACTIVE_ATTEMPT_ERROR_CODE: &str =
    "CODEY_SUBAGENT_FOLLOWUP_REQUIRES_ACTIVE_ATTEMPT";
const UNBOUND_ATTEMPT_ERROR_CODE: &str = "CODEY_SUBAGENT_UNBOUND_ATTEMPT";
const AGENT_ID_COLLISION_ERROR_CODE: &str = "CODEY_SUBAGENT_AGENT_ID_COLLISION";
const CONCURRENCY_LIMIT_ERROR_CODE: &str = "CODEY_SUBAGENT_CONCURRENCY_LIMIT";
const MAX_CLAIMS_PER_MODE: usize = 16;
const MAX_ACCEPTANCE_CHECKS: usize = 8;
const MAX_ACCEPTANCE_COMMAND_CHARS: usize = 1024;
const MAX_ACCEPTANCE_TOTAL_CHARS: usize = 4 * 1024;
const MAX_CONTRACT_LINE_CHARS: usize = 8 * 1024;
const MAX_REASON_CHARS: usize = 128;
const MAX_BATCH_DECISION_REASON_CHARS: usize = 512;
const MAX_BATCH_DECISION_ID_CHARS: usize = 128;
const MAX_BATCH_DECISION_IDS: usize = 32;
const BATCH_DECISION_CONTROL_FAILURE_ERROR_CODE: &str = "CODEY_SUBAGENT_CONTROL_PLANE_FAILED";
const MAX_BATCH_DECISION_CONTROL_FAILURES: u16 = 3;
const BATCH_DECISION_CONTROL_FAILURE_GRACE_MILLIS: u64 = 10 * 60 * 1000;
const MAX_SCHEMA_BYTES: usize = 4 * 1024;
const MAX_SCHEMA_DEPTH: usize = 16;
const MAX_ACCEPTANCE_FAILURES: u16 = 3;
const MAX_UNCHANGED_ACCEPTANCE_STOPS: u16 = 3;
const ACCEPTANCE_STALL_GRACE_MILLIS: u64 = 10 * 60 * 1000;
const LEDGER_LOCK_TIMEOUT_MILLIS: u64 = 250;
const LEDGER_LOCK_RETRY_MILLIS: u64 = 5;
const SETTLEMENT_RECEIPT_PREFIX: &str = "orchestrator-settlement-v1";
const MAX_TRANSCRIPT_METADATA_LINE_BYTES: usize = 1024 * 1024;
const MAX_SPAWN_RESPONSE_JSON_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DelegationContract {
    id: String,
    #[serde(rename = "why")]
    reason: String,
    #[serde(default)]
    visual: bool,
    #[serde(default, rename = "root")]
    workspace_root: Option<String>,
    #[serde(default, rename = "read")]
    read_paths: Vec<String>,
    #[serde(default, rename = "write")]
    write_paths: Vec<String>,
    #[serde(default, rename = "checks")]
    acceptance: Vec<AcceptanceSpec>,
    #[serde(default)]
    mode: InvocationMode,
    #[serde(default)]
    trace_id: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    deadline_ms: Option<u64>,
    #[serde(default)]
    input_schema: Option<Value>,
    #[serde(default)]
    output_schema: Option<Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptanceSpec {
    id: String,
    #[serde(rename = "cmd")]
    command: String,
}

#[derive(Clone, Debug)]
struct PreparedContract {
    contract: DelegationContract,
    role: String,
    policy: RolePolicy,
    workspace_root: Option<String>,
    read_paths: Vec<String>,
    native_read_scope: bool,
    write_paths: Vec<String>,
    trace: TraceContext,
    invocation_mode: InvocationMode,
    capabilities: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SessionLedger {
    schema_version: u32,
    runtime_id_hash: String,
    session_id_hash: String,
    revision: u64,
    created_at_ms: u64,
    updated_at_ms: u64,
    #[serde(default = "default_batch_number")]
    batch_number: u16,
    #[serde(default)]
    issued_task_ids: BTreeSet<String>,
    #[serde(default = "default_fencing_token")]
    next_fencing_token: u64,
    #[serde(default)]
    decision_required: bool,
    #[serde(default)]
    batch_decision: BatchDecisionState,
    #[serde(default)]
    used_decision_ids: BTreeSet<String>,
    #[serde(default)]
    batch_decision_control_failure_count: u16,
    #[serde(default)]
    batch_decision_control_failure_started_at_ms: Option<u64>,
    reservations: BTreeMap<String, Reservation>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RootBatchDecision {
    SpawnNextBatch,
    ContinueRoot,
    Complete,
    Blocked,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum BatchDecisionControlFailureKind {
    InvalidReceipt,
    NoProgress,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum BatchDecisionState {
    #[default]
    None,
    Awaiting {
        batch_number: u16,
        opened_at_ms: u64,
    },
    Pending {
        batch_number: u16,
        decision: RootBatchDecision,
        decision_id: String,
        reason_hash: String,
        prepared_at_ms: u64,
    },
    Committed {
        batch_number: u16,
        decision: RootBatchDecision,
        decision_id: String,
        reason_hash: String,
        committed_at_ms: u64,
    },
    ControlPlaneFailed {
        batch_number: u16,
        failure_kind: BatchDecisionControlFailureKind,
        failure_count: u16,
        failed_at_ms: u64,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchDecisionInput {
    decision: RootBatchDecision,
    batch_number: u16,
    decision_id: String,
    reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Reservation {
    task_id: String,
    #[serde(default = "default_batch_number")]
    batch_number: u16,
    role: String,
    write_capable: bool,
    visual: bool,
    workspace_root: Option<String>,
    read_paths: Vec<String>,
    #[serde(default)]
    native_read_scope: bool,
    write_paths: Vec<String>,
    state: ReservationState,
    #[serde(default)]
    outcome: ExecutionOutcome,
    agent_id_hash: Option<String>,
    created_at_ms: u64,
    updated_at_ms: u64,
    acceptance: Vec<AcceptanceEntry>,
    #[serde(default)]
    trace_id: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    invocation_mode: InvocationMode,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    input_schema: Option<Value>,
    #[serde(default)]
    output_schema: Option<Value>,
    #[serde(default)]
    started_at_ms: Option<u64>,
    #[serde(default)]
    completed_at_ms: Option<u64>,
    #[serde(default)]
    token_usage: Option<TokenUsage>,
    #[serde(default)]
    error_message: Option<String>,
    #[serde(default)]
    deadline_at_ms: Option<u64>,
    #[serde(default)]
    attempt_id: String,
    #[serde(default)]
    fencing_token: u64,
    #[serde(default)]
    policy_revision: u64,
    #[serde(default)]
    fenced_at_ms: Option<u64>,
    #[serde(default)]
    spawn_failed: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AcceptanceEntry {
    id: String,
    command: String,
    command_hash: String,
    status: AcceptanceStatus,
    attempted_at_ms: Option<u64>,
    evidence_hash: Option<String>,
    #[serde(default)]
    failure_count: u16,
    #[serde(default)]
    blocked_stop_count: u16,
    #[serde(default)]
    blocked_since_ms: Option<u64>,
    #[serde(default)]
    release_notice_delivered_at_ms: Option<u64>,
    #[serde(default)]
    release_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct SettlementReceipt {
    schema_version: u32,
    runtime_id_hash: String,
    session_id_hash: String,
    ledger_created_at_ms: u64,
    settled_at_ms: u64,
    batch_number: u16,
    final_decision: String,
    unverifiable_acceptance: Vec<UnverifiableAcceptanceReceipt>,
}

#[derive(Clone, Debug, Serialize)]
struct UnverifiableAcceptanceReceipt {
    task_id: String,
    check_id: String,
    failure_count: u16,
    reason: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AcceptanceStatus {
    Pending,
    Passed,
    Failed,
    Unverifiable,
}

const fn default_batch_number() -> u16 {
    1
}

const fn default_fencing_token() -> u64 {
    1
}

struct LedgerStore {
    lock: File,
    ledger_path: PathBuf,
}

fn ledger_lock_is_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        // LockFileEx reports ERROR_LOCK_VIOLATION for an occupied byte range.
        || (cfg!(windows) && error.raw_os_error() == Some(33))
}

impl LedgerStore {
    fn open(state_root: &Path, session_id: &str) -> Result<Self> {
        fs::create_dir_all(state_root).with_context(|| {
            format!(
                "创建 Codey 子代理编排状态目录失败：{}",
                state_root.display()
            )
        })?;
        let session_hash = hash_component(session_id);
        let lock_path = state_root.join(format!("{LEDGER_LOCK_FILE}.{session_hash}"));
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("打开 Codey 子代理编排账本锁失败：{}", lock_path.display()))?;
        let lock_started = Instant::now();
        loop {
            match lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(error) if ledger_lock_is_contended(&error) => {
                    if lock_started.elapsed() >= Duration::from_millis(LEDGER_LOCK_TIMEOUT_MILLIS) {
                        anyhow::bail!(
                            "获取 Codey 子代理编排账本锁超时（{} ms）：{}",
                            LEDGER_LOCK_TIMEOUT_MILLIS,
                            lock_path.display()
                        );
                    }
                    thread::sleep(Duration::from_millis(LEDGER_LOCK_RETRY_MILLIS));
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("获取 Codey 子代理编排账本锁失败：{}", lock_path.display())
                    });
                }
            }
        }
        let session_dir = state_root.join(session_hash);
        Ok(Self {
            lock,
            ledger_path: session_dir.join(LEDGER_FILE),
        })
    }

    fn load(
        &self,
        runtime_id: &str,
        session_id: &str,
        now_ms: u64,
    ) -> Result<Option<SessionLedger>> {
        let bytes = match fs::read(&self.ledger_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "读取 Codey 子代理编排账本失败：{}",
                        self.ledger_path.display()
                    )
                });
            }
        };
        let mut ledger: SessionLedger = serde_json::from_slice(&bytes).with_context(|| {
            format!(
                "解析 Codey 子代理编排账本失败：{}",
                self.ledger_path.display()
            )
        })?;
        anyhow::ensure!(
            (MIN_LEDGER_SCHEMA_VERSION..=LEDGER_SCHEMA_VERSION).contains(&ledger.schema_version),
            "Codey 子代理编排账本版本不受支持：{}",
            ledger.schema_version
        );
        let source_schema_version = ledger.schema_version;
        let mut changed = migrate_ledger(&mut ledger, source_schema_version)?;
        let session_id_hash = hash_component(session_id);
        anyhow::ensure!(
            ledger.session_id_hash == session_id_hash,
            "Codey 子代理编排账本会话标识不一致"
        );
        changed |= expire_reservations(&mut ledger, now_ms);
        let runtime_id_hash = hash_component(runtime_id);
        if ledger.runtime_id_hash != runtime_id_hash {
            ledger.reservations.retain(|_, reservation| {
                reservation.write_capable
                    && !reservation.spawn_failed
                    && reservation_has_pending_acceptance(reservation)
            });
            for reservation in ledger.reservations.values_mut() {
                reservation.batch_number = 1;
                reservation.state = ReservationState::Recovered;
                if reservation.outcome == ExecutionOutcome::Unknown {
                    reservation.outcome = ExecutionOutcome::Lost;
                }
                reservation.agent_id_hash = None;
                reservation.updated_at_ms = now_ms;
                reservation.completed_at_ms.get_or_insert(now_ms);
                reservation.fenced_at_ms.get_or_insert(now_ms);
                reservation.error_message.get_or_insert_with(|| {
                    "runtime generation changed before an authoritative successful outcome"
                        .to_string()
                });
            }
            ledger.runtime_id_hash = runtime_id_hash;
            ledger.batch_number = 1;
            ledger.issued_task_ids = ledger.reservations.keys().cloned().collect();
            ledger.decision_required = false;
            ledger.batch_decision = BatchDecisionState::None;
            ledger.used_decision_ids.clear();
            reset_batch_decision_control_failures(&mut ledger);
            ledger.updated_at_ms = now_ms;
            changed = true;
        }
        validate_unique_agent_bindings(&ledger)?;
        if changed {
            self.save(&mut ledger, now_ms)?;
        }
        Ok(Some(ledger))
    }

    fn save(&self, ledger: &mut SessionLedger, now_ms: u64) -> Result<()> {
        validate_unique_agent_bindings(ledger)?;
        let parent = self
            .ledger_path
            .parent()
            .context("Codey 子代理编排账本缺少父目录")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("创建 Codey 子代理编排账本目录失败：{}", parent.display()))?;
        ledger.revision = ledger.revision.saturating_add(1);
        ledger.updated_at_ms = now_ms;
        let bytes = serde_json::to_vec(ledger).context("序列化 Codey 子代理编排账本失败")?;
        crate::fs_util::atomic_write(&self.ledger_path, &bytes).with_context(|| {
            format!(
                "原子替换 Codey 子代理编排账本失败：{}",
                self.ledger_path.display()
            )
        })
    }

    fn write_settlement_receipt(
        &self,
        ledger: &SessionLedger,
        runtime_id: &str,
        session_id: &str,
    ) -> Result<()> {
        let parent = self
            .ledger_path
            .parent()
            .context("Codey 子代理结算回执缺少父目录")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("创建 Codey 子代理结算回执目录失败：{}", parent.display()))?;
        let final_decision = match &ledger.batch_decision {
            BatchDecisionState::Committed { decision, .. } => match decision {
                RootBatchDecision::SpawnNextBatch => "spawn_next_batch",
                RootBatchDecision::ContinueRoot => "continue_root",
                RootBatchDecision::Complete => "complete",
                RootBatchDecision::Blocked => "blocked",
            },
            BatchDecisionState::ControlPlaneFailed { .. } => "control_plane_failed",
            _ => "none",
        };
        let settled_at_ms = match &ledger.batch_decision {
            BatchDecisionState::Committed {
                committed_at_ms, ..
            } => *committed_at_ms,
            BatchDecisionState::ControlPlaneFailed { failed_at_ms, .. } => *failed_at_ms,
            _ => ledger.updated_at_ms,
        };
        let mut unverifiable_acceptance = Vec::new();
        for reservation in ledger.reservations.values() {
            for check in &reservation.acceptance {
                if check.status == AcceptanceStatus::Unverifiable {
                    unverifiable_acceptance.push(UnverifiableAcceptanceReceipt {
                        task_id: reservation.task_id.clone(),
                        check_id: check.id.clone(),
                        failure_count: check.failure_count,
                        reason: check
                            .release_reason
                            .clone()
                            .unwrap_or_else(|| "无法取得可信的验收证据".to_string()),
                    });
                }
            }
        }
        let receipt = SettlementReceipt {
            schema_version: 1,
            runtime_id_hash: hash_component(runtime_id),
            session_id_hash: hash_component(session_id),
            ledger_created_at_ms: ledger.created_at_ms,
            settled_at_ms,
            batch_number: ledger.batch_number,
            final_decision: final_decision.to_string(),
            unverifiable_acceptance,
        };
        let bytes = serde_json::to_vec(&receipt).context("序列化 Codey 子代理结算回执失败")?;
        let digest = hash_component_bytes(&bytes);
        let receipt_path = parent.join(format!(
            "{SETTLEMENT_RECEIPT_PREFIX}-{}-{}.json",
            ledger.created_at_ms,
            &digest[..16]
        ));
        if receipt_path.exists() {
            anyhow::ensure!(
                fs::read(&receipt_path).ok().as_deref() == Some(bytes.as_slice()),
                "Codey 子代理结算回执目标冲突：{}",
                receipt_path.display()
            );
            return Ok(());
        }
        crate::fs_util::atomic_write(&receipt_path, &bytes)
            .with_context(|| format!("写入 Codey 子代理结算回执失败：{}", receipt_path.display()))
    }

    fn remove(&self) -> Result<()> {
        match fs::remove_file(&self.ledger_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "删除 Codey 子代理编排账本失败：{}",
                        self.ledger_path.display()
                    )
                });
            }
        }
        if let Some(parent) = self.ledger_path.parent() {
            match fs::remove_dir(parent) {
                Ok(()) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                    ) => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("清理 Codey 子代理编排账本目录失败：{}", parent.display())
                    });
                }
            }
        }
        Ok(())
    }

    // SessionEnd 不带 runtime 归属信息；代次不一致且仍有未清偿预留/验收债时
    // 保留账本，交给所属代次或恢复逻辑处理，其余情况照常删除。
    fn remove_for_session_end(
        &self,
        runtime_id: &str,
        session_id: &str,
        now_ms: u64,
    ) -> Result<()> {
        let bytes = match fs::read(&self.ledger_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return self.remove(),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "读取 Codey 子代理编排账本失败：{}",
                        self.ledger_path.display()
                    )
                });
            }
        };
        let mut ledger = match serde_json::from_slice::<SessionLedger>(&bytes) {
            Ok(ledger) if ledger.session_id_hash == hash_component(session_id) => ledger,
            Ok(_) => {
                self.quarantine_for_session_end(&bytes, "会话标识不一致")?;
                return Ok(());
            }
            Err(error) => {
                self.quarantine_for_session_end(&bytes, &format!("JSON 无法解析：{error}"))?;
                return Ok(());
            }
        };
        if ledger_has_outstanding(&ledger) && ledger.runtime_id_hash != hash_component(runtime_id) {
            return Ok(());
        }
        if ledger_has_outstanding(&ledger) {
            let active_tasks = ledger
                .reservations
                .iter()
                .filter(|(_, reservation)| reservation.state.is_active())
                .map(|(task_id, _)| task_id.clone())
                .collect::<BTreeSet<_>>();
            if !active_tasks.is_empty() {
                fence_identity_conflict(
                    &mut ledger,
                    &active_tasks,
                    now_ms,
                    "SessionEnd arrived before an authoritative terminal outcome",
                );
            }
            self.save(&mut ledger, now_ms)?;
            return Ok(());
        }
        self.remove()
    }

    fn quarantine_for_session_end(&self, bytes: &[u8], reason: &str) -> Result<()> {
        let digest = hash_component_bytes(bytes);
        let quarantine_path = self.ledger_path.with_file_name(format!(
            "orchestrator-ledger-v1.corrupt-{}.json",
            &digest[..16]
        ));
        if quarantine_path.exists() {
            anyhow::ensure!(
                fs::read(&quarantine_path).ok().as_deref() == Some(bytes),
                "Codey 子代理损坏账本隔离目标冲突：{}",
                quarantine_path.display()
            );
            fs::remove_file(&self.ledger_path).with_context(|| {
                format!(
                    "移除已留存副本的 Codey 子代理损坏账本失败：{}",
                    self.ledger_path.display()
                )
            })?;
        } else {
            fs::rename(&self.ledger_path, &quarantine_path).with_context(|| {
                format!(
                    "隔离 Codey 子代理损坏账本失败：{} -> {}",
                    self.ledger_path.display(),
                    quarantine_path.display()
                )
            })?;
        }
        eprintln!(
            "Codey SessionEnd 已隔离不可读的子代理账本（{reason}）：{}",
            quarantine_path.display()
        );
        Ok(())
    }
}

impl Drop for LedgerStore {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.lock);
    }
}

impl SessionLedger {
    fn new(runtime_id: &str, session_id: &str, now_ms: u64) -> Self {
        Self {
            schema_version: LEDGER_SCHEMA_VERSION,
            runtime_id_hash: hash_component(runtime_id),
            session_id_hash: hash_component(session_id),
            revision: 0,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            batch_number: 1,
            issued_task_ids: BTreeSet::new(),
            next_fencing_token: 1,
            decision_required: false,
            batch_decision: BatchDecisionState::None,
            used_decision_ids: BTreeSet::new(),
            batch_decision_control_failure_count: 0,
            batch_decision_control_failure_started_at_ms: None,
            reservations: BTreeMap::new(),
        }
    }
}

fn migrate_ledger(ledger: &mut SessionLedger, source_schema_version: u32) -> Result<bool> {
    let mut changed = false;
    if ledger.batch_number == 0 {
        ledger.batch_number = 1;
        changed = true;
    }
    if source_schema_version < 3 {
        for reservation in ledger.reservations.values_mut() {
            reservation.batch_number = 1;
        }
        ledger.batch_number = 1;
        ledger.issued_task_ids = ledger.reservations.keys().cloned().collect();
        changed = true;
    } else {
        let before = ledger.issued_task_ids.len();
        ledger
            .issued_task_ids
            .extend(ledger.reservations.keys().cloned());
        changed |= ledger.issued_task_ids.len() != before;
    }
    if source_schema_version < 4 {
        let session_hash = ledger.session_id_hash.clone();
        for (task_id, reservation) in &mut ledger.reservations {
            if reservation.trace_id.is_empty() {
                reservation.trace_id = hash_component(&format!("{session_hash}:{task_id}"));
            }
            reservation.started_at_ms = match reservation.state {
                ReservationState::Running
                | ReservationState::Terminal
                | ReservationState::Recovered => Some(reservation.updated_at_ms),
                ReservationState::Pending | ReservationState::Failed => None,
            };
            reservation.completed_at_ms = matches!(
                reservation.state,
                ReservationState::Terminal | ReservationState::Failed | ReservationState::Recovered
            )
            .then_some(reservation.updated_at_ms);
        }
        changed = true;
    }
    if source_schema_version < 5 {
        let session_hash = ledger.session_id_hash.clone();
        let mut next_fencing_token = 1_u64;
        for (task_id, reservation) in &mut ledger.reservations {
            if reservation.attempt_id.is_empty() {
                reservation.attempt_id = hash_component(&format!(
                    "{session_hash}:{task_id}:{}:{}",
                    reservation.batch_number, reservation.created_at_ms
                ));
            }
            if reservation.fencing_token == 0 {
                reservation.fencing_token = next_fencing_token;
            }
            next_fencing_token = next_fencing_token
                .max(reservation.fencing_token)
                .saturating_add(1);
            match reservation.state {
                ReservationState::Failed => {
                    reservation.state = ReservationState::Terminal;
                    reservation.outcome = ExecutionOutcome::Failed;
                    reservation.spawn_failed = true;
                    reservation
                        .completed_at_ms
                        .get_or_insert(reservation.updated_at_ms);
                    reservation
                        .fenced_at_ms
                        .get_or_insert(reservation.updated_at_ms);
                }
                ReservationState::Recovered => {
                    if reservation.outcome == ExecutionOutcome::Unknown {
                        reservation.outcome = ExecutionOutcome::Lost;
                    }
                    reservation
                        .fenced_at_ms
                        .get_or_insert(reservation.updated_at_ms);
                }
                ReservationState::Terminal => {
                    // Schema v1-v4 did not persist an authoritative outcome. In
                    // particular, errored/shutdown/not_found were folded into
                    // the same phase as completed, so migration must not infer
                    // success from the old terminal bit.
                    reservation.outcome = ExecutionOutcome::Unknown;
                    reservation
                        .fenced_at_ms
                        .get_or_insert(reservation.updated_at_ms);
                }
                ReservationState::Pending | ReservationState::Running => {}
            }
        }
        ledger.next_fencing_token = ledger.next_fencing_token.max(next_fencing_token);
        changed = true;
    } else {
        let required_next = ledger
            .reservations
            .values()
            .map(|reservation| reservation.fencing_token)
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .max(1);
        if ledger.next_fencing_token < required_next {
            ledger.next_fencing_token = required_next;
            changed = true;
        }
    }
    if source_schema_version < 6 {
        // Existing in-flight ledgers retain the legacy lazy-transition behavior.
        // The first spawn admitted by this runtime opts the ledger into the
        // explicit decision protocol, avoiding an upgrade-time deadlock.
        ledger.decision_required = false;
        ledger.batch_decision = BatchDecisionState::None;
        ledger.used_decision_ids.clear();
        changed = true;
    }
    if source_schema_version < 7 {
        ledger.batch_decision_control_failure_count = 0;
        ledger.batch_decision_control_failure_started_at_ms = None;
        changed = true;
    }
    anyhow::ensure!(
        ledger.used_decision_ids.len() <= MAX_BATCH_DECISION_IDS,
        "Codey 子代理编排账本批次决策 ID 数量无效：{}",
        ledger.used_decision_ids.len()
    );
    for reservation in ledger.reservations.values() {
        anyhow::ensure!(
            reservation.fencing_token > 0 && !reservation.attempt_id.is_empty(),
            "Codey 子代理编排账本缺少有效 attempt/fencing 元数据"
        );
        anyhow::ensure!(
            reservation.state.is_settled() || reservation.outcome == ExecutionOutcome::Unknown,
            "Codey 子代理编排账本的活动 phase 带有终态 outcome"
        );
    }
    if ledger.schema_version != LEDGER_SCHEMA_VERSION {
        ledger.schema_version = LEDGER_SCHEMA_VERSION;
        changed = true;
    }
    Ok(changed)
}

fn expire_reservations(ledger: &mut SessionLedger, now_ms: u64) -> bool {
    let mut changed = false;
    for reservation in ledger.reservations.values_mut() {
        if reservation.state.is_active()
            && reservation
                .deadline_at_ms
                .is_some_and(|deadline_at_ms| now_ms >= deadline_at_ms)
        {
            reservation.state = ReservationState::Terminal;
            reservation.outcome = ExecutionOutcome::TimedOut;
            reservation.agent_id_hash = None;
            reservation.updated_at_ms = now_ms;
            reservation.completed_at_ms = Some(now_ms);
            reservation.fenced_at_ms = Some(now_ms);
            reservation.error_message = Some(format!(
                "execution deadline expired at {}",
                reservation.deadline_at_ms.unwrap_or(now_ms)
            ));
            changed = true;
        }
    }
    changed
}

fn current_batch_is_settled(ledger: &SessionLedger) -> bool {
    let mut found = false;
    for reservation in ledger
        .reservations
        .values()
        .filter(|reservation| reservation.batch_number == ledger.batch_number)
    {
        found = true;
        if !reservation.state.is_settled() {
            return false;
        }
    }
    found
}

fn current_batch_has_admitted_agent(ledger: &SessionLedger) -> bool {
    ledger.reservations.values().any(|reservation| {
        reservation.batch_number == ledger.batch_number && !reservation.spawn_failed
    })
}

fn ensure_awaiting_batch_decision(
    ledger: &mut SessionLedger,
    active_agents: usize,
    now_ms: u64,
) -> bool {
    if !ledger.decision_required
        || active_agents != 0
        || !current_batch_is_settled(ledger)
        || !current_batch_has_admitted_agent(ledger)
        || !matches!(ledger.batch_decision, BatchDecisionState::None)
    {
        return false;
    }
    ledger.batch_decision = BatchDecisionState::Awaiting {
        batch_number: ledger.batch_number,
        opened_at_ms: now_ms,
    };
    true
}

fn start_next_batch(ledger: &mut SessionLedger) {
    ledger.batch_number = ledger.batch_number.saturating_add(1);
    ledger.batch_decision = BatchDecisionState::None;
    ledger.used_decision_ids.clear();
    reset_batch_decision_control_failures(ledger);
}

fn reset_batch_decision_control_failures(ledger: &mut SessionLedger) {
    ledger.batch_decision_control_failure_count = 0;
    ledger.batch_decision_control_failure_started_at_ms = None;
}

fn observe_batch_decision_control_failure(
    ledger: &mut SessionLedger,
    failure_kind: BatchDecisionControlFailureKind,
    now_ms: u64,
) -> bool {
    let started_at_ms = *ledger
        .batch_decision_control_failure_started_at_ms
        .get_or_insert(now_ms);
    ledger.batch_decision_control_failure_count = ledger
        .batch_decision_control_failure_count
        .saturating_add(1);
    let failure_count = ledger.batch_decision_control_failure_count;
    let exhausted = failure_count >= MAX_BATCH_DECISION_CONTROL_FAILURES
        || now_ms.saturating_sub(started_at_ms) >= BATCH_DECISION_CONTROL_FAILURE_GRACE_MILLIS;
    if exhausted {
        ledger.batch_decision = BatchDecisionState::ControlPlaneFailed {
            batch_number: ledger.batch_number,
            failure_kind,
            failure_count,
            failed_at_ms: now_ms,
        };
    }
    exhausted
}

fn batch_decision_control_failure_reason(ledger: &SessionLedger) -> String {
    let (failure_kind, failure_count) = match &ledger.batch_decision {
        BatchDecisionState::ControlPlaneFailed {
            failure_kind,
            failure_count,
            ..
        } => (*failure_kind, *failure_count),
        _ => (
            BatchDecisionControlFailureKind::NoProgress,
            ledger.batch_decision_control_failure_count,
        ),
    };
    let failure_detail = match failure_kind {
        BatchDecisionControlFailureKind::InvalidReceipt => "工具回执持续无效",
        BatchDecisionControlFailureKind::NoProgress => "批次决策持续无进展",
    };
    format!(
        "{BATCH_DECISION_CONTROL_FAILURE_ERROR_CODE}: Codey 第 {} 批决策控制面连续失败 {} 次（{failure_detail}），已按保守策略终止本轮批次决策。不会授权新的子代理或普通根工具；请立即 Stop，并在最终答复中报告该错误码。",
        ledger.batch_number, failure_count
    )
}

fn advance_batch_if_settled(
    ledger: &mut SessionLedger,
    active_agents: usize,
    now_ms: u64,
) -> (bool, Option<String>) {
    if active_agents != 0 || !current_batch_is_settled(ledger) {
        return (false, None);
    }
    if !current_batch_has_admitted_agent(ledger) {
        return (false, None);
    }
    if !ledger.decision_required {
        start_next_batch(ledger);
        return (true, None);
    }

    let opened = ensure_awaiting_batch_decision(ledger, active_agents, now_ms);
    match &ledger.batch_decision {
        BatchDecisionState::Committed {
            decision: RootBatchDecision::SpawnNextBatch,
            ..
        } => {
            start_next_batch(ledger);
            (true, None)
        }
        _ => (opened, Some(batch_decision_spawn_denial(ledger))),
    }
}

fn batch_decision_spawn_denial(ledger: &SessionLedger) -> String {
    if matches!(
        ledger.batch_decision,
        BatchDecisionState::ControlPlaneFailed { .. }
    ) {
        return batch_decision_control_failure_reason(ledger);
    }
    let detail = match &ledger.batch_decision {
        BatchDecisionState::Awaiting { .. } => "尚未提交批次决策",
        BatchDecisionState::Pending { .. } => "批次决策仍在等待工具成功回执",
        BatchDecisionState::Committed {
            decision: RootBatchDecision::ContinueRoot,
            ..
        } => "已提交 `continue_root`，它不授权派生下一批",
        BatchDecisionState::Committed {
            decision: RootBatchDecision::Complete,
            ..
        } => "已提交 `complete`，它不授权派生下一批",
        BatchDecisionState::Committed {
            decision: RootBatchDecision::Blocked,
            ..
        } => "已提交 `blocked`，它不授权派生下一批",
        BatchDecisionState::Committed {
            decision: RootBatchDecision::SpawnNextBatch,
            ..
        } => "下一批授权尚未被消费",
        BatchDecisionState::ControlPlaneFailed { .. } => {
            "批次决策控制面已失败关闭，不能再派生下一批"
        }
        BatchDecisionState::None => "尚未打开批次决策窗口",
    };
    format!(
        "Codey 批次决策门禁：第 {} 批已经终态，但{detail}。请先调用 `{}`，显式选择 `spawn_next_batch`、`continue_root`、`complete` 或 `blocked`；只有 `spawn_next_batch` 成功提交后才能调用 `agents.spawn_agent`。",
        ledger.batch_number,
        crate::subagent_control_mcp::QUALIFIED_TOOL_NAME
    )
}

fn concurrency_denial(
    ledger: &SessionLedger,
    prepared: &PreparedContract,
    active_agents: usize,
) -> Option<String> {
    let tracked_active = ledger
        .reservations
        .values()
        .filter(|reservation| reservation.state.is_active() && !reservation.spawn_failed)
        .collect::<Vec<_>>();
    let tracked_active_count = tracked_active.len();
    let has_untracked_active = active_agents > tracked_active_count;
    let has_active_write = has_untracked_active
        || tracked_active
            .iter()
            .any(|reservation| reservation.write_capable);
    let candidate_is_read_only = prepared.policy.access == RoleAccess::ReadOnly;
    let limit = if candidate_is_read_only && !has_active_write {
        READ_ONLY_CONCURRENCY_LIMIT
    } else {
        WRITE_OR_MIXED_CONCURRENCY_LIMIT
    };
    let observed_active = active_agents.max(tracked_active_count);
    if observed_active < limit {
        return None;
    }
    let mode = if limit == READ_ONLY_CONCURRENCY_LIMIT {
        "已确认的纯只读批次"
    } else {
        "包含写入型或身份未确认代理的批次"
    };
    Some(format!(
        "{CONCURRENCY_LIMIT_ERROR_CODE}: Codey 子代理并发门禁：{mode}当前已有 {observed_active} 个活动代理，达到并发上限 {limit}。请先等待任一活动代理进入终态后再派发；该限制只约束同时运行数量，不限制后续批次或累计派发次数。"
    ))
}

#[cfg(test)]
pub(crate) fn pre_spawn(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    tool_input: Option<&Value>,
    active_agents: usize,
    now_ms: u64,
) -> Result<Option<String>> {
    // 测试契约统一声明 root "/repo"，这里模拟 Hook 提供同等工作目录；
    // 需要覆盖 cwd 缺失场景的测试请直接调用 pre_spawn_with_workspace。
    pre_spawn_with_workspace(
        state_root,
        runtime_id,
        session_id,
        tool_input,
        Some("/repo"),
        active_agents,
        now_ms,
    )
}

pub(crate) fn pre_spawn_with_workspace(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    tool_input: Option<&Value>,
    hook_workspace_root: Option<&str>,
    active_agents: usize,
    now_ms: u64,
) -> Result<Option<String>> {
    let loaded_rules = rules::load(state_root);
    if let Some(warning) = &loaded_rules.warning {
        eprintln!("Codey 子代理规则回退：{warning}");
    }
    let prepared =
        match prepare_contract_with_rules(tool_input, hook_workspace_root, &loaded_rules.rules) {
            Ok(prepared) => prepared,
            Err(reason) => return Ok(Some(reason)),
        };
    let store = LedgerStore::open(state_root, session_id)?;
    let mut ledger = store
        .load(runtime_id, session_id, now_ms)?
        .unwrap_or_else(|| SessionLedger::new(runtime_id, session_id, now_ms));

    if active_agents == 0
        && ledger.reservations.values().any(|reservation| {
            matches!(
                reservation.state,
                ReservationState::Terminal | ReservationState::Recovered
            ) && reservation_has_pending_acceptance(reservation)
        })
    {
        return Ok(Some(
            "Codey 自适应委派门禁：上一批写入任务仍有未清偿的机械验收债；请先执行 Stop 提示中的精确验收命令。"
                .to_string(),
        ));
    }
    if ledger.issued_task_ids.contains(&prepared.contract.id) {
        return Ok(Some(duplicate_task_id_denial(
            &ledger,
            &prepared.contract.id,
        )));
    }
    if let Some(conflict) = resource_conflict(&prepared, &ledger) {
        return Ok(Some(conflict));
    }
    let (batch_changed, batch_denial) =
        advance_batch_if_settled(&mut ledger, active_agents, now_ms);
    if let Some(reason) = batch_denial {
        if batch_changed {
            store.save(&mut ledger, now_ms)?;
        }
        return Ok(Some(reason));
    }

    if let Some(reason) = concurrency_denial(&ledger, &prepared, active_agents) {
        if batch_changed {
            store.save(&mut ledger, now_ms)?;
        }
        return Ok(Some(reason));
    }

    ledger.decision_required = true;
    ledger.issued_task_ids.insert(prepared.contract.id.clone());
    let acceptance = prepared
        .contract
        .acceptance
        .iter()
        .map(|check| AcceptanceEntry {
            id: check.id.clone(),
            command: check.command.trim().to_string(),
            command_hash: hash_component(check.command.trim()),
            status: AcceptanceStatus::Pending,
            attempted_at_ms: None,
            evidence_hash: None,
            failure_count: 0,
            blocked_stop_count: 0,
            blocked_since_ms: None,
            release_notice_delivered_at_ms: None,
            release_reason: None,
        })
        .collect();
    let trace = prepared.trace.clone();
    let task_id = prepared.contract.id.clone();
    let role = prepared.role.clone();
    let deadline_at_ms = prepared
        .contract
        .deadline_ms
        .map(|duration_ms| now_ms.saturating_add(duration_ms));
    let fencing_token = ledger.next_fencing_token;
    ledger.next_fencing_token = ledger.next_fencing_token.saturating_add(1);
    let attempt_id = hash_component(&format!(
        "{}:{}:{}:{}:{}",
        hash_component(runtime_id),
        hash_component(session_id),
        task_id,
        ledger.batch_number,
        fencing_token
    ));
    let policy_revision = loaded_rules.rules.revision;
    let input_schema = prepared.contract.input_schema.clone();
    let output_schema = prepared.contract.output_schema.clone();
    let input_schema_hash = input_schema.as_ref().map(canonical_value_hash);
    let output_schema_hash = output_schema.as_ref().map(canonical_value_hash);
    ledger.reservations.insert(
        prepared.contract.id.clone(),
        Reservation {
            task_id: prepared.contract.id,
            batch_number: ledger.batch_number,
            role: prepared.role,
            write_capable: prepared.policy.access == RoleAccess::Write,
            visual: prepared.policy.visual,
            workspace_root: prepared.workspace_root,
            read_paths: prepared.read_paths,
            native_read_scope: prepared.native_read_scope,
            write_paths: prepared.write_paths,
            state: ReservationState::Pending,
            outcome: ExecutionOutcome::Unknown,
            agent_id_hash: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            acceptance,
            trace_id: prepared.trace.trace_id,
            parent_id: prepared.trace.parent_id,
            invocation_mode: prepared.invocation_mode,
            capabilities: prepared.capabilities,
            input_schema,
            output_schema,
            started_at_ms: None,
            completed_at_ms: None,
            token_usage: None,
            error_message: None,
            deadline_at_ms,
            attempt_id: attempt_id.clone(),
            fencing_token,
            policy_revision,
            fenced_at_ms: None,
            spawn_failed: false,
        },
    );
    store.save(&mut ledger, now_ms)?;
    let mut event = SubagentTraceEvent::new(
        now_ms,
        &trace,
        TraceEventKind::Scheduled,
        ExecutionStatus::Pending,
        runtime_id,
        session_id,
        &task_id,
        None,
        Some(&role),
    );
    event.attributes.insert(
        "invocation.mode".into(),
        serde_json::to_value(prepared.invocation_mode).unwrap_or(Value::Null),
    );
    event.attributes.insert(
        "rules.source".into(),
        Value::String(format!("{:?}", loaded_rules.source).to_ascii_lowercase()),
    );
    event.attributes.insert(
        "rules.revision".into(),
        Value::Number(loaded_rules.rules.revision.into()),
    );
    event
        .attributes
        .insert("attempt.id".into(), Value::String(attempt_id));
    event
        .attributes
        .insert("fencing.token".into(), Value::Number(fencing_token.into()));
    if let Some(deadline_at_ms) = deadline_at_ms {
        event.attributes.insert(
            "execution.deadline_at_ms".into(),
            Value::Number(deadline_at_ms.into()),
        );
    }
    if let Some(schema_hash) = input_schema_hash {
        event
            .attributes
            .insert("input.schema_hash".into(), Value::String(schema_hash));
    }
    if let Some(schema_hash) = output_schema_hash {
        event
            .attributes
            .insert("output.schema_hash".into(), Value::String(schema_hash));
    }
    TraceRecorder::new(state_root).record_best_effort(&event);
    Ok(None)
}

pub(crate) fn post_spawn(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    tool_input: Option<&Value>,
    tool_response: Option<&Value>,
    now_ms: u64,
) -> Result<()> {
    let Some(task_id) = spawn_task_id(tool_input) else {
        return Ok(());
    };
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(mut ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(());
    };
    let Some(reservation) = ledger.reservations.get(task_id) else {
        return Ok(());
    };
    if reservation.state != ReservationState::Pending {
        return Ok(());
    }
    let returned_agent_id =
        tool_response.and_then(|response| extract_spawn_binding_identifier(response, task_id));
    if let Some(agent_id) = returned_agent_id.as_deref() {
        let mut conflicts = identity_task_candidates(&ledger, agent_id);
        conflicts.remove(task_id);
        if !conflicts.is_empty() {
            conflicts.insert(task_id.to_string());
            let reason = format!(
                "spawn 回执 agent_id 与其他 attempt 冲突；关联任务：{}",
                conflicts.iter().cloned().collect::<Vec<_>>().join(", ")
            );
            fence_identity_conflict(&mut ledger, &conflicts, now_ms, &reason);
            store.save(&mut ledger, now_ms)?;
            anyhow::bail!(
                "{AGENT_ID_COLLISION_ERROR_CODE}: {reason}。所有相关活动 attempt 已被 fence，禁止复用该身份"
            );
        }
    }
    let reservation = ledger
        .reservations
        .get_mut(task_id)
        .expect("reservation checked above");
    let trace = reservation_trace(reservation);
    let role = reservation.role.clone();
    let mut trace_event = None;
    if returned_agent_id.is_none() && tool_response.is_some_and(response_is_explicit_failure) {
        reservation.state = ReservationState::Terminal;
        reservation.outcome = ExecutionOutcome::Failed;
        reservation.spawn_failed = true;
        reservation.fenced_at_ms = Some(now_ms);
        reservation.updated_at_ms = now_ms;
        reservation.completed_at_ms = Some(now_ms);
        reservation.token_usage = telemetry::extract_token_usage(tool_response);
        reservation.error_message = Some("spawn tool reported failure".to_string());
        let mut event = SubagentTraceEvent::new(
            now_ms,
            &trace,
            TraceEventKind::Failed,
            ExecutionStatus::Failed,
            runtime_id,
            session_id,
            task_id,
            None,
            Some(&role),
        );
        event.latency_ms = Some(now_ms.saturating_sub(reservation.created_at_ms));
        event.usage = reservation.token_usage.clone();
        event.error_code = Some("spawn_failed".into());
        event.error_message = reservation.error_message.clone();
        trace_event = Some(event);
    } else if let Some(agent_id) = returned_agent_id.as_deref() {
        reservation.state = ReservationState::Running;
        reservation.outcome = ExecutionOutcome::Unknown;
        reservation.updated_at_ms = now_ms;
        reservation.started_at_ms = Some(now_ms);
        reservation.agent_id_hash = Some(hash_component(agent_id));
        let mut event = SubagentTraceEvent::new(
            now_ms,
            &trace,
            TraceEventKind::Started,
            ExecutionStatus::Running,
            runtime_id,
            session_id,
            task_id,
            Some(agent_id),
            Some(&role),
        );
        event.latency_ms = Some(now_ms.saturating_sub(reservation.created_at_ms));
        trace_event = Some(event);
    } else {
        // 没有明确失败，也没有可绑定的代理 ID，只能确认工具调用已经返回，不能确认
        // 子代理是否真正创建。保留 Pending，等待生命周期事件或完整状态快照对账。
        reservation.updated_at_ms = now_ms;
    }
    store.save(&mut ledger, now_ms)?;
    if let Some(event) = trace_event {
        TraceRecorder::new(state_root).record_best_effort(&event);
    }
    Ok(())
}

pub(crate) fn pre_followup_task(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    tool_input: Option<&Value>,
    now_ms: u64,
) -> Result<Option<String>> {
    let Some(target) = followup_task_target(tool_input) else {
        return Ok(Some(format!(
            "{FOLLOWUP_REQUIRES_ACTIVE_ATTEMPT_ERROR_CODE}: Codey 生命周期门禁：`agents.followup_task` 缺少非空 target，已在唤醒子代理前拒绝。不要重试本次调用；请修正目标，或由主代理接管。"
        )));
    };
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(Some(followup_without_active_attempt_denial(
            target,
            "当前会话没有可验证的活动委派账本",
        )));
    };
    let Some(task_id) = unique_task_for_identifier(&ledger, target)? else {
        return Ok(Some(followup_without_active_attempt_denial(
            target,
            "target 无法匹配当前账本中的 reservation",
        )));
    };
    let reservation = &ledger.reservations[&task_id];
    if reservation.state == ReservationState::Running
        && reservation.agent_id_hash.is_some()
        && reservation.fenced_at_ms.is_none()
        && !reservation.spawn_failed
    {
        return Ok(None);
    }
    if reservation.state == ReservationState::Pending && reservation.fenced_at_ms.is_none() {
        return Ok(Some(format!(
            "{FOLLOWUP_REQUIRES_ACTIVE_ATTEMPT_ERROR_CODE}: Codey 生命周期门禁：目标 `{target}` 的 attempt `{}` 仍为 pending，尚未绑定 agent_id；已在唤醒子代理前拒绝。不要重试 `followup_task`，请先调用一次不带筛选的 `agents.list_agents` 对账并继续等待。若快照明确没有匹配代理，由主代理接管；只有范围实质改变且仍值得委派时，才使用全新的 task_name 和完全相同的新 `CODEY_DELEGATION_V2.id` 调用 `agents.spawn_agent`。",
            reservation.attempt_id
        )));
    }
    Ok(Some(followup_without_active_attempt_denial(
        target,
        &format!(
            "匹配 attempt `{}` 已不可恢复（state={:?}, outcome={:?}）",
            reservation.attempt_id, reservation.state, reservation.outcome
        ),
    )))
}

fn followup_without_active_attempt_denial(target: &str, detail: &str) -> String {
    format!(
        "{FOLLOWUP_REQUIRES_ACTIVE_ATTEMPT_ERROR_CODE}: Codey 生命周期门禁：`agents.followup_task` 只能用于当前会话中仍为 running、已绑定 agent_id 且未被 fence 的 attempt；目标 `{target}` 不满足条件（{detail}），已在唤醒子代理前拒绝。不要重试 `followup_task`，也不要等待旧 canonical task 自行恢复。若仍有独立工作，使用全新的 task_name 和完全相同的新 `CODEY_DELEGATION_V2.id` 调用 `agents.spawn_agent`；若上一批已结算，先显式选择 `spawn_next_batch`。否则由主代理立即接管。"
    )
}

fn duplicate_task_id_denial(ledger: &SessionLedger, task_id: &str) -> String {
    let prefix = format!(
        "{DUPLICATE_TASK_ID_ERROR_CODE}: Codey 自适应委派门禁：任务 ID `{task_id}` 已在本轮编排账本中，禁止重复派生。"
    );
    match ledger.reservations.get(task_id) {
        Some(reservation) if reservation.state == ReservationState::Pending => format!(
            "{prefix} 账本状态为 `pending`，上次派生结果尚未确认。不要重发旧 ID，也不要把本次拒绝当作完成后立即 Stop；先调用一次不带筛选的 `agents.list_agents` 对账。若找到对应代理，等待其进入终态或消费已有终态结果；若明确没有匹配代理，默认由主代理接管。只有任务范围确实缩小或改变且仍值得委派时，才可用全新的 `task_name` 最多重试一次，并把 `CODEY_DELEGATION_V2.id` 同步改为完全相同的新值。禁止改走 `functions.exec` 重试派生。"
        ),
        Some(reservation) if reservation.state == ReservationState::Running => format!(
            "{prefix} 账本状态为 `running`，原派生已经建立。不要重发旧 ID，也不要直接 Stop；先调用一次不带筛选的 `agents.list_agents` 对账。若代理仍为 `running`、`pending_init` 或 `interrupted`，继续等待或必要协调；若已终态，消费已有结果；若快照明确没有匹配代理，由主代理接管。禁止改走 `functions.exec` 重试派生。"
        ),
        Some(reservation) if reservation.spawn_failed => format!(
            "{prefix} 账本状态为 `failed`，上次派生已明确失败，成本点虽已退还但尝试次数仍计入。不要再次使用旧 ID，也不要把本次拒绝当作完成后立即 Stop；默认由主代理接管。只有任务范围确实缩小或改变且仍值得委派时，才可用全新的 `task_name` 最多重试一次，并把 `CODEY_DELEGATION_V2.id` 同步改为完全相同的新值。禁止改走 `functions.exec` 重试派生。"
        ),
        Some(reservation) => format!(
            "{prefix} 原任务已经进入终态或恢复态（outcome={:?}），不得重新派生；请先消费已有结果并完成仍需执行的机械验收，不要把本次拒绝当作新结果后立即 Stop。",
            reservation.outcome
        ),
        None => format!(
            "{prefix} 当前无法从兼容账本恢复原 reservation。不要重发旧 ID，也不要立即 Stop；先调用一次不带筛选的 `agents.list_agents` 对账。若找到对应代理，等待或消费其结果；若明确没有匹配代理，默认由主代理接管。只有任务范围确实改变且仍值得委派时，才可用全新的 `task_name` 最多重试一次，并同步更新 `CODEY_DELEGATION_V2.id`。"
        ),
    }
}

#[cfg(test)]
pub(crate) fn subagent_started(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    agent_id: &str,
    now_ms: u64,
) -> Result<()> {
    subagent_started_with_role(state_root, runtime_id, session_id, agent_id, None, now_ms)
}

#[cfg(test)]
pub(crate) fn subagent_started_with_role(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    agent_id: &str,
    agent_type: Option<&str>,
    now_ms: u64,
) -> Result<()> {
    subagent_started_with_context(
        state_root, runtime_id, session_id, agent_id, agent_type, None, now_ms,
    )
}

pub(crate) fn subagent_started_with_context(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    agent_id: &str,
    agent_type: Option<&str>,
    transcript_path: Option<&str>,
    now_ms: u64,
) -> Result<()> {
    update_reservation_lifecycle(
        state_root,
        runtime_id,
        session_id,
        LifecycleContext {
            agent_id,
            agent_type,
            transcript_path,
            state: ReservationState::Running,
        },
        now_ms,
    )
}

#[cfg(test)]
pub(crate) fn subagent_stopped(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    agent_id: &str,
    now_ms: u64,
) -> Result<()> {
    subagent_stopped_with_context(
        state_root, runtime_id, session_id, agent_id, None, None, now_ms,
    )
}

pub(crate) fn subagent_stopped_with_context(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    agent_id: &str,
    agent_type: Option<&str>,
    transcript_path: Option<&str>,
    now_ms: u64,
) -> Result<()> {
    update_reservation_lifecycle(
        state_root,
        runtime_id,
        session_id,
        LifecycleContext {
            agent_id,
            agent_type,
            transcript_path,
            state: ReservationState::Terminal,
        },
        now_ms,
    )
}

struct LifecycleContext<'a> {
    agent_id: &'a str,
    agent_type: Option<&'a str>,
    transcript_path: Option<&'a str>,
    state: ReservationState,
}

fn update_reservation_lifecycle(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    context: LifecycleContext<'_>,
    now_ms: u64,
) -> Result<()> {
    let LifecycleContext {
        agent_id,
        agent_type,
        transcript_path,
        state,
    } = context;
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(mut ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(());
    };
    let agent_hash = hash_component(agent_id);
    let mut candidates = identity_task_candidates(&ledger, agent_id);
    if candidates.is_empty()
        && let Some(task_id) = task_id_from_subagent_transcript(
            state_root,
            session_id,
            agent_id,
            agent_type,
            transcript_path,
            &ledger,
        )
    {
        candidates.insert(task_id);
    }
    if candidates.len() > 1 {
        let reason = format!(
            "生命周期标识 `{agent_id}` 同时指向多个 attempt：{}",
            candidates.iter().cloned().collect::<Vec<_>>().join(", ")
        );
        fence_identity_conflict(&mut ledger, &candidates, now_ms, &reason);
        store.save(&mut ledger, now_ms)?;
        anyhow::bail!("{AGENT_ID_COLLISION_ERROR_CODE}: {reason}");
    }
    let Some(task_id) = candidates.into_iter().next() else {
        return Ok(());
    };
    if let Some(role) = agent_type
        && ledger
            .reservations
            .get(&task_id)
            .is_none_or(|reservation| reservation.role != role)
    {
        let reason = format!("生命周期事件角色 `{role}` 与 attempt `{task_id}` 的绑定角色不一致");
        fence_identity_conflict(
            &mut ledger,
            &BTreeSet::from([task_id.clone()]),
            now_ms,
            &reason,
        );
        store.save(&mut ledger, now_ms)?;
        anyhow::bail!("{AGENT_ID_COLLISION_ERROR_CODE}: {reason}");
    }
    if let Some(bound_hash) = ledger
        .reservations
        .get(&task_id)
        .and_then(|reservation| reservation.agent_id_hash.as_deref())
        && bound_hash != agent_hash
        && !is_provisional_task_binding(bound_hash, &task_id)
    {
        let reason = format!("生命周期 agent_id 与 attempt `{task_id}` 的既有运行时身份不一致");
        fence_identity_conflict(
            &mut ledger,
            &BTreeSet::from([task_id.clone()]),
            now_ms,
            &reason,
        );
        store.save(&mut ledger, now_ms)?;
        anyhow::bail!("{AGENT_ID_COLLISION_ERROR_CODE}: {reason}");
    }
    let mut trace_event = None;
    if let Some(reservation) = ledger.reservations.get_mut(&task_id) {
        if reservation.state == state {
            if reservation.agent_id_hash.as_deref() != Some(agent_hash.as_str()) {
                reservation.agent_id_hash = Some(agent_hash);
                reservation.updated_at_ms = now_ms;
                if state == ReservationState::Running {
                    reservation.started_at_ms.get_or_insert(now_ms);
                }
                store.save(&mut ledger, now_ms)?;
            }
            return Ok(());
        }
        if reservation.state.transition_to(state).is_none() {
            return Ok(());
        }
        reservation.state = state;
        reservation.agent_id_hash = Some(agent_hash);
        reservation.updated_at_ms = now_ms;
        let (event_kind, status) = match state {
            ReservationState::Running => {
                reservation.outcome = ExecutionOutcome::Unknown;
                reservation.started_at_ms.get_or_insert(now_ms);
                (TraceEventKind::Started, ExecutionStatus::Running)
            }
            ReservationState::Terminal => {
                reservation.outcome = ExecutionOutcome::Unknown;
                reservation.completed_at_ms = Some(now_ms);
                reservation.fenced_at_ms = Some(now_ms);
                reservation.error_message = Some(
                    "lifecycle stop did not include an authoritative execution outcome".to_string(),
                );
                (TraceEventKind::Failed, ExecutionStatus::Failed)
            }
            ReservationState::Failed => {
                reservation.outcome = ExecutionOutcome::Failed;
                reservation.completed_at_ms = Some(now_ms);
                reservation.fenced_at_ms = Some(now_ms);
                (TraceEventKind::Failed, ExecutionStatus::Failed)
            }
            ReservationState::Recovered => {
                reservation.outcome = ExecutionOutcome::Lost;
                reservation.completed_at_ms = Some(now_ms);
                reservation.fenced_at_ms = Some(now_ms);
                (TraceEventKind::Recovered, ExecutionStatus::Recovered)
            }
            ReservationState::Pending => (TraceEventKind::Scheduled, ExecutionStatus::Pending),
        };
        let trace = reservation_trace(reservation);
        let mut event = SubagentTraceEvent::new(
            now_ms,
            &trace,
            event_kind,
            status,
            runtime_id,
            session_id,
            &task_id,
            Some(agent_id),
            Some(&reservation.role),
        );
        event.latency_ms = Some(now_ms.saturating_sub(reservation.created_at_ms));
        event.usage = reservation.token_usage.clone();
        event.attributes.insert(
            "execution.outcome".into(),
            Value::String(format!("{:?}", reservation.outcome).to_ascii_lowercase()),
        );
        if state == ReservationState::Terminal {
            event.error_code = Some("unknown_terminal_outcome".into());
            event.error_message = reservation.error_message.clone();
        }
        trace_event = Some(event);
    }
    store.save(&mut ledger, now_ms)?;
    if let Some(event) = trace_event {
        TraceRecorder::new(state_root).record_best_effort(&event);
    }
    Ok(())
}

pub(crate) fn observe_status_response(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    tool_response: Option<&Value>,
    all_terminal: bool,
    now_ms: u64,
) -> Result<()> {
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(mut ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(());
    };
    let mut terminal_tasks = BTreeMap::new();
    if let Some(response) = tool_response
        && let Err(error) =
            collect_terminal_task_outcomes(response, &mut ledger, &mut terminal_tasks, now_ms)
    {
        store.save(&mut ledger, now_ms)?;
        return Err(error);
    }
    if all_terminal {
        for (task_id, reservation) in &ledger.reservations {
            if reservation.state.is_active() {
                terminal_tasks
                    .entry(task_id.clone())
                    .or_insert(ExecutionOutcome::Lost);
            }
        }
    }
    let mut changed = false;
    let usage = telemetry::extract_token_usage(tool_response);
    let mut trace_events = Vec::new();
    for (task_id, outcome) in terminal_tasks {
        let Some(reservation) = ledger.reservations.get_mut(&task_id) else {
            continue;
        };
        let transitions_to_terminal = reservation.state != ReservationState::Terminal
            && reservation
                .state
                .transition_to(ReservationState::Terminal)
                .is_some();
        let refines_lifecycle_stop = reservation.state == ReservationState::Terminal
            && reservation.outcome == ExecutionOutcome::Unknown;
        if transitions_to_terminal || refines_lifecycle_stop {
            reservation.state = ReservationState::Terminal;
            reservation.outcome = outcome;
            reservation.fenced_at_ms.get_or_insert(now_ms);
            reservation.agent_id_hash = None;
            reservation.updated_at_ms = now_ms;
            reservation.completed_at_ms.get_or_insert(now_ms);
            reservation.error_message = match outcome {
                ExecutionOutcome::Succeeded => None,
                ExecutionOutcome::Failed => {
                    Some("authoritative agent status reported failure".into())
                }
                ExecutionOutcome::TimedOut => Some("authoritative agent status timed out".into()),
                ExecutionOutcome::Lost => Some(
                    "authoritative status snapshot settled the attempt without a successful result"
                        .into(),
                ),
                ExecutionOutcome::Unknown => Some("terminal outcome was not recognized".into()),
            };
            if usage.is_some() {
                reservation.token_usage = usage.clone();
            }
            let trace = reservation_trace(reservation);
            let success = outcome.is_success();
            let mut event = SubagentTraceEvent::new(
                now_ms,
                &trace,
                if success {
                    TraceEventKind::Completed
                } else {
                    TraceEventKind::Failed
                },
                if success {
                    ExecutionStatus::Succeeded
                } else {
                    ExecutionStatus::Failed
                },
                runtime_id,
                session_id,
                &task_id,
                None,
                Some(&reservation.role),
            );
            event.latency_ms = Some(now_ms.saturating_sub(reservation.created_at_ms));
            event.usage = reservation.token_usage.clone();
            event.attributes.insert(
                "execution.outcome".into(),
                Value::String(format!("{outcome:?}").to_ascii_lowercase()),
            );
            if !success {
                event.error_code = Some(
                    match outcome {
                        ExecutionOutcome::Failed => "agent_failed",
                        ExecutionOutcome::TimedOut => "agent_timed_out",
                        ExecutionOutcome::Lost => "agent_lost",
                        ExecutionOutcome::Unknown => "unknown_terminal_outcome",
                        ExecutionOutcome::Succeeded => unreachable!(),
                    }
                    .into(),
                );
                event.error_message = reservation.error_message.clone();
            }
            trace_events.push(event);
            changed = true;
        }
    }
    if changed {
        store.save(&mut ledger, now_ms)?;
        let recorder = TraceRecorder::new(state_root);
        for event in &trace_events {
            recorder.record_best_effort(event);
        }
    }
    Ok(())
}

/// Returns the lifecycle ledger projection when a session ledger exists.
/// Active-marker files remain a migration fallback in the gate, but they are
/// no longer the primary source of truth for newly scheduled work.
pub(crate) fn active_reservation_count(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    now_ms: u64,
) -> Result<Option<usize>> {
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(None);
    };
    Ok(Some(
        ledger
            .reservations
            .values()
            .filter(|reservation| reservation.state.is_active())
            .count(),
    ))
}

/// Atomically fences every still-active reservation before the gate discards
/// legacy marker files. This keeps the ledger as the authoritative source of
/// truth and prevents a Stop recovery loop from resurrecting stale work.
pub(crate) fn recover_active_reservations(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    reason: &str,
    now_ms: u64,
) -> Result<usize> {
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(mut ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(0);
    };
    let mut recovered = 0_usize;
    for reservation in ledger.reservations.values_mut() {
        if !reservation.state.is_active() {
            continue;
        }
        reservation.state = ReservationState::Recovered;
        reservation.outcome = ExecutionOutcome::Lost;
        reservation.agent_id_hash = None;
        reservation.updated_at_ms = now_ms;
        reservation.completed_at_ms = Some(now_ms);
        reservation.fenced_at_ms = Some(now_ms);
        reservation.error_message = Some(reason.to_string());
        recovered = recovered.saturating_add(1);
    }
    if recovered > 0 {
        store.save(&mut ledger, now_ms)?;
    }
    Ok(recovered)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AbandonedReservation {
    /// Hash used by the legacy active-marker filename. The ledger remains the
    /// source of truth, but returning it lets the gate remove the migration
    /// fallback without retaining a raw provider identifier.
    pub(crate) agent_id_hash: Option<String>,
    pub(crate) changed: bool,
}

/// Permanently abandons exactly the reservation named by a successful root
/// `interrupt_agent` call. The upstream interrupted state is intentionally
/// resumable; Codey's root-level interrupt means this task is no longer wanted,
/// so the local attempt is fenced before root work is allowed to continue.
pub(crate) fn abandon_interrupted_reservation(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    tool_input: Option<&Value>,
    now_ms: u64,
) -> Result<Option<AbandonedReservation>> {
    let Some(target) = interrupt_task_target(tool_input) else {
        return Ok(None);
    };
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(mut ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(None);
    };
    let Some(task_id) = unique_task_for_identifier(&ledger, &target)? else {
        return Ok(None);
    };
    let reservation = ledger
        .reservations
        .get_mut(&task_id)
        .expect("resolved reservation must exist");
    let agent_id_hash = reservation.agent_id_hash.clone();
    if !reservation.state.is_active() {
        return Ok(Some(AbandonedReservation {
            agent_id_hash,
            changed: false,
        }));
    }

    let trace = reservation_trace(reservation);
    let role = reservation.role.clone();
    reservation.state = ReservationState::Recovered;
    reservation.outcome = ExecutionOutcome::Lost;
    reservation.agent_id_hash = None;
    reservation.updated_at_ms = now_ms;
    reservation.completed_at_ms = Some(now_ms);
    reservation.fenced_at_ms = Some(now_ms);
    reservation.error_message =
        Some("root successfully interrupted and permanently abandoned this task".to_string());
    store.save(&mut ledger, now_ms)?;

    let mut event = SubagentTraceEvent::new(
        now_ms,
        &trace,
        TraceEventKind::Recovered,
        ExecutionStatus::Recovered,
        runtime_id,
        session_id,
        &task_id,
        Some(&target),
        Some(&role),
    );
    event.error_code = Some("root_interrupt_abandoned".into());
    event.error_message =
        Some("root successfully interrupted and permanently abandoned this task".to_string());
    TraceRecorder::new(state_root).record_best_effort(&event);

    Ok(Some(AbandonedReservation {
        agent_id_hash,
        changed: true,
    }))
}

pub(crate) fn open_batch_decision_if_settled(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    active_agents: usize,
    now_ms: u64,
) -> Result<Option<String>> {
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(mut ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(None);
    };
    if ensure_awaiting_batch_decision(&mut ledger, active_agents, now_ms) {
        store.save(&mut ledger, now_ms)?;
    }
    Ok(
        matches!(ledger.batch_decision, BatchDecisionState::Awaiting { .. })
            .then(|| batch_decision_continuation(&ledger)),
    )
}

pub(crate) fn prepare_batch_decision(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    tool_input: Option<&Value>,
    active_agents: usize,
    now_ms: u64,
) -> Result<Option<String>> {
    if active_agents != 0 {
        return Ok(Some(format!(
            "Codey 批次决策门禁：仍有 {active_agents} 个子代理未终态；请先完成 `agents.wait_agent`/`agents.list_agents` 对账。"
        )));
    }
    let input = match parse_batch_decision_input(tool_input) {
        Ok(input) => input,
        Err(reason) => return Ok(Some(reason)),
    };
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(mut ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(Some(
            "Codey 批次决策门禁：当前会话没有可决策的子代理批次。".to_string(),
        ));
    };
    let opened = ensure_awaiting_batch_decision(&mut ledger, active_agents, now_ms);
    if matches!(
        ledger.batch_decision,
        BatchDecisionState::ControlPlaneFailed { .. }
    ) {
        return Ok(Some(batch_decision_control_failure_reason(&ledger)));
    }
    if input.batch_number != ledger.batch_number {
        if opened {
            store.save(&mut ledger, now_ms)?;
        }
        return Ok(Some(format!(
            "Codey 批次决策门禁：输入批次为 {}，当前批次为 {}；请使用门禁提示中的当前批次号。",
            input.batch_number, ledger.batch_number
        )));
    }
    if batch_decision_state_matches(&ledger.batch_decision, &input) {
        if opened {
            store.save(&mut ledger, now_ms)?;
        }
        return Ok(None);
    }
    if ledger.used_decision_ids.contains(&input.decision_id) {
        if opened {
            store.save(&mut ledger, now_ms)?;
        }
        return Ok(Some(format!(
            "Codey 批次决策门禁：decision_id `{}` 已用于不同决策；请生成新的唯一 ID。",
            input.decision_id
        )));
    }
    if ledger.used_decision_ids.len() >= MAX_BATCH_DECISION_IDS {
        if opened {
            store.save(&mut ledger, now_ms)?;
        }
        return Ok(Some(
            "Codey 批次决策门禁：本轮决策 ID 数量达到上限；请由主代理报告 blocked 并停止继续切换决策。"
                .to_string(),
        ));
    }
    if matches!(ledger.batch_decision, BatchDecisionState::None) {
        return Ok(Some(
            "Codey 批次决策门禁：当前批次尚未全部进入终态，不能提前提交决策。".to_string(),
        ));
    }
    if input.decision == RootBatchDecision::Complete && ledger_has_unverifiable_acceptance(&ledger)
    {
        if opened {
            store.save(&mut ledger, now_ms)?;
        }
        return Ok(Some(
            "Codey 机械验收门禁：当前批次包含无法验证的验收项，不能提交 `complete`；请提交 `blocked`，并在最终答复中报告未验证项及其原因。"
                .to_string(),
        ));
    }

    let reason_hash = hash_component(input.reason.trim());
    ledger.used_decision_ids.insert(input.decision_id.clone());
    ledger.batch_decision = BatchDecisionState::Pending {
        batch_number: input.batch_number,
        decision: input.decision,
        decision_id: input.decision_id,
        reason_hash,
        prepared_at_ms: now_ms,
    };
    store.save(&mut ledger, now_ms)?;
    Ok(None)
}

pub(crate) fn post_batch_decision(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    tool_input: Option<&Value>,
    tool_response: Option<&Value>,
    now_ms: u64,
) -> Result<Option<String>> {
    let input = match parse_batch_decision_input(tool_input) {
        Ok(input) => input,
        Err(reason) => return Ok(Some(reason)),
    };
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(mut ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(Some(
            "Codey 批次决策门禁：工具返回后找不到对应会话账本，决策未提交。".to_string(),
        ));
    };
    if matches!(&ledger.batch_decision, BatchDecisionState::Committed { .. })
        && batch_decision_state_matches(&ledger.batch_decision, &input)
    {
        return Ok(None);
    }
    if matches!(
        ledger.batch_decision,
        BatchDecisionState::ControlPlaneFailed { .. }
    ) {
        return Ok(Some(batch_decision_control_failure_reason(&ledger)));
    }
    if !matches!(&ledger.batch_decision, BatchDecisionState::Pending { .. })
        || !batch_decision_state_matches(&ledger.batch_decision, &input)
    {
        return Ok(Some(
            "Codey 批次决策门禁：工具回执与已准备的决策不一致，已拒绝提交。".to_string(),
        ));
    }
    if !tool_response.is_some_and(|response| decision_receipt_matches(response, &input)) {
        ledger.used_decision_ids.remove(&input.decision_id);
        ledger.batch_decision = BatchDecisionState::Awaiting {
            batch_number: ledger.batch_number,
            opened_at_ms: now_ms,
        };
        let control_plane_failed = observe_batch_decision_control_failure(
            &mut ledger,
            BatchDecisionControlFailureKind::InvalidReceipt,
            now_ms,
        );
        store.save(&mut ledger, now_ms)?;
        if control_plane_failed {
            return Ok(Some(batch_decision_control_failure_reason(&ledger)));
        }
        return Ok(Some(format!(
            "Codey 批次决策门禁：`{}` 未返回匹配的 accepted 回执，决策未提交；请检查工具错误后用相同或新的 decision_id 重试（连续失败 {}/{} 次）。",
            crate::subagent_control_mcp::QUALIFIED_TOOL_NAME,
            ledger.batch_decision_control_failure_count,
            MAX_BATCH_DECISION_CONTROL_FAILURES
        )));
    }

    reset_batch_decision_control_failures(&mut ledger);
    ledger.batch_decision = BatchDecisionState::Committed {
        batch_number: input.batch_number,
        decision: input.decision,
        decision_id: input.decision_id,
        reason_hash: hash_component(input.reason.trim()),
        committed_at_ms: now_ms,
    };
    store.save(&mut ledger, now_ms)?;
    Ok(None)
}

pub(crate) fn batch_decision_stop_reason(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    now_ms: u64,
) -> Result<Option<String>> {
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(mut ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(None);
    };
    let opened = ensure_awaiting_batch_decision(&mut ledger, 0, now_ms);
    let should_observe_failure = matches!(
        ledger.batch_decision,
        BatchDecisionState::Awaiting { .. }
            | BatchDecisionState::Pending { .. }
            | BatchDecisionState::Committed {
                decision: RootBatchDecision::ContinueRoot | RootBatchDecision::SpawnNextBatch,
                ..
            }
    );
    let control_plane_failed = should_observe_failure
        && observe_batch_decision_control_failure(
            &mut ledger,
            BatchDecisionControlFailureKind::NoProgress,
            now_ms,
        );
    if opened || should_observe_failure {
        store.save(&mut ledger, now_ms)?;
    }
    if control_plane_failed {
        return Ok(None);
    }
    let reason = match &ledger.batch_decision {
        BatchDecisionState::None
        | BatchDecisionState::ControlPlaneFailed { .. }
        | BatchDecisionState::Committed {
            decision: RootBatchDecision::Complete | RootBatchDecision::Blocked,
            ..
        } => None,
        BatchDecisionState::Awaiting { .. } | BatchDecisionState::Pending { .. } => {
            Some(batch_decision_continuation(&ledger))
        }
        BatchDecisionState::Committed {
            decision: RootBatchDecision::ContinueRoot,
            ..
        } => Some(format!(
            "Codey 批次决策门禁：第 {} 批已选择 `continue_root`，主代理尚未提交最终 `complete` 或 `blocked` 决策；若仍需委派也可改为 `spawn_next_batch`。请先再次调用 `{}`。",
            ledger.batch_number,
            crate::subagent_control_mcp::QUALIFIED_TOOL_NAME
        )),
        BatchDecisionState::Committed {
            decision: RootBatchDecision::SpawnNextBatch,
            ..
        } => Some(format!(
            "Codey 批次决策门禁：第 {} 批已授权 `spawn_next_batch`，但授权尚未通过一次真实 `agents.spawn_agent` 消费；请派发下一批，或用新的 decision_id 改为 `complete`/`blocked`。",
            ledger.batch_number
        )),
    };
    Ok(reason)
}

fn batch_decision_root_tool_reason(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    tool_input: Option<&Value>,
    now_ms: u64,
) -> Result<Option<String>> {
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(None);
    };
    let is_acceptance_command = extract_command(tool_input)
        .and_then(parse_acceptance_marker)
        .is_some();
    if is_acceptance_command {
        return Ok(None);
    }
    let reason = match &ledger.batch_decision {
        BatchDecisionState::None
        | BatchDecisionState::Committed {
            decision: RootBatchDecision::ContinueRoot,
            ..
        } => None,
        BatchDecisionState::Awaiting { .. } | BatchDecisionState::Pending { .. } => {
            Some(batch_decision_continuation(&ledger))
        }
        BatchDecisionState::Committed {
            decision: RootBatchDecision::SpawnNextBatch,
            ..
        } => Some(
            "Codey 批次决策门禁：已选择 `spawn_next_batch`；下一步只能直接调用 `agents.spawn_agent` 消费授权，或提交新的批次决策。"
                .to_string(),
        ),
        BatchDecisionState::Committed {
            decision: RootBatchDecision::Complete | RootBatchDecision::Blocked,
            ..
        } => Some(
            "Codey 批次决策门禁：已选择结束本轮；除精确的机械验收命令外，不能再执行普通根代理工具。若仍需工作，请用新的 decision_id 改为 `continue_root`。"
                .to_string(),
        ),
        BatchDecisionState::ControlPlaneFailed { .. } => {
            Some(batch_decision_control_failure_reason(&ledger))
        }
    };
    Ok(reason)
}

fn parse_batch_decision_input(
    tool_input: Option<&Value>,
) -> std::result::Result<BatchDecisionInput, String> {
    let mut input = serde_json::from_value::<BatchDecisionInput>(
        tool_input
            .cloned()
            .ok_or_else(|| "Codey 批次决策门禁：缺少工具输入。".to_string())?,
    )
    .map_err(|error| format!("Codey 批次决策门禁：工具输入无效：{error}"))?;
    input.decision_id = input.decision_id.trim().to_string();
    input.reason = input.reason.trim().to_string();
    if input.decision_id.is_empty()
        || input.decision_id.len() > MAX_BATCH_DECISION_ID_CHARS
        || !input
            .decision_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
    {
        return Err(
            "Codey 批次决策门禁：decision_id 必须为 1-128 个 ASCII 字母、数字或 `-_.:`。"
                .to_string(),
        );
    }
    if input.reason.is_empty() || input.reason.chars().count() > MAX_BATCH_DECISION_REASON_CHARS {
        return Err("Codey 批次决策门禁：reason 必须为 1-512 个字符。".to_string());
    }
    Ok(input)
}

fn batch_decision_state_matches(state: &BatchDecisionState, input: &BatchDecisionInput) -> bool {
    match state {
        BatchDecisionState::Pending {
            batch_number,
            decision,
            decision_id,
            reason_hash,
            ..
        }
        | BatchDecisionState::Committed {
            batch_number,
            decision,
            decision_id,
            reason_hash,
            ..
        } => {
            *batch_number == input.batch_number
                && *decision == input.decision
                && decision_id == &input.decision_id
                && reason_hash == &hash_component(input.reason.trim())
        }
        BatchDecisionState::None
        | BatchDecisionState::Awaiting { .. }
        | BatchDecisionState::ControlPlaneFailed { .. } => false,
    }
}

fn decision_receipt_matches(value: &Value, input: &BatchDecisionInput) -> bool {
    let Some(envelope) = value.as_object() else {
        return false;
    };
    if envelope.get("isError").and_then(Value::as_bool) == Some(true) {
        return false;
    }

    let result = envelope
        .get("result")
        .and_then(Value::as_object)
        .unwrap_or(envelope);
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return false;
    }

    let Some(receipt) = result.get("structuredContent").and_then(Value::as_object) else {
        return false;
    };
    let expected_decision = match input.decision {
        RootBatchDecision::SpawnNextBatch => "spawn_next_batch",
        RootBatchDecision::ContinueRoot => "continue_root",
        RootBatchDecision::Complete => "complete",
        RootBatchDecision::Blocked => "blocked",
    };
    receipt.get("accepted").and_then(Value::as_bool) == Some(true)
        && receipt.get("batch_number").and_then(Value::as_u64)
            == Some(u64::from(input.batch_number))
        && receipt.get("decision_id").and_then(Value::as_str) == Some(input.decision_id.as_str())
        && receipt.get("reason").and_then(Value::as_str) == Some(input.reason.as_str())
        && receipt.get("decision").and_then(Value::as_str) == Some(expected_decision)
}

fn batch_decision_continuation(ledger: &SessionLedger) -> String {
    format!(
        "Codey 批次决策门禁：第 {} 批已全部进入终态。请现在调用 `{}`，使用唯一 decision_id 和简短 reason，显式选择 `spawn_next_batch`、`continue_root`、`complete` 或 `blocked`。不要等待下一条用户消息，也不要跳过决策直接 Stop。",
        ledger.batch_number,
        crate::subagent_control_mcp::QUALIFIED_TOOL_NAME
    )
}

#[cfg(test)]
pub(crate) fn authorize_child_tool(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    agent_id: &str,
    tool_name: &str,
    tool_input: Option<&Value>,
    now_ms: u64,
) -> Result<Option<String>> {
    authorize_child_tool_with_context(
        state_root,
        runtime_id,
        session_id,
        ChildToolContext {
            agent_id,
            agent_type: None,
            transcript_path: None,
            tool_name,
            tool_input,
        },
        now_ms,
    )
}

pub(crate) struct ChildToolContext<'a> {
    pub(crate) agent_id: &'a str,
    pub(crate) agent_type: Option<&'a str>,
    pub(crate) transcript_path: Option<&'a str>,
    pub(crate) tool_name: &'a str,
    pub(crate) tool_input: Option<&'a Value>,
}

pub(crate) fn authorize_child_tool_with_context(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    context: ChildToolContext<'_>,
    now_ms: u64,
) -> Result<Option<String>> {
    let ChildToolContext {
        agent_id,
        agent_type,
        transcript_path,
        tool_name,
        tool_input,
    } = context;
    let loaded_rules = rules::load(state_root);
    if let Some(warning) = &loaded_rules.warning {
        eprintln!("Codey 子代理规则回退：{warning}");
    }
    let tool_class = rules::classify_tool(tool_name);
    let store = LedgerStore::open(state_root, session_id)?;
    let mut ledger = store.load(runtime_id, session_id, now_ms)?;
    let agent_hash = hash_component(agent_id);
    let mut bound_task = None;
    if let Some(current) = ledger.as_ref() {
        let candidates = identity_task_candidates(current, agent_id);
        if candidates.len() > 1 {
            let reason = format!(
                "{AGENT_ID_COLLISION_ERROR_CODE}: child 标识 `{agent_id}` 同时指向多个 attempt（{}）；相关活动权限已被 fence",
                candidates.iter().cloned().collect::<Vec<_>>().join(", ")
            );
            if let Some(current) = ledger.as_mut() {
                fence_identity_conflict(current, &candidates, now_ms, &reason);
                store.save(current, now_ms)?;
            }
            return Ok(Some(reason));
        }
        bound_task = candidates.into_iter().next();
        if bound_task.is_none() {
            bound_task = task_id_from_subagent_transcript(
                state_root,
                session_id,
                agent_id,
                agent_type,
                transcript_path,
                current,
            );
        }
    }
    if let (Some(expected_role), Some(task_id), Some(current)) =
        (agent_type, bound_task.as_deref(), ledger.as_ref())
        && current
            .reservations
            .get(task_id)
            .is_some_and(|reservation| reservation.role != expected_role)
    {
        let reason = format!(
            "{AGENT_ID_COLLISION_ERROR_CODE}: child 上报角色 `{expected_role}` 与 attempt `{task_id}` 的绑定角色不一致；该 attempt 已被 fence。"
        );
        if let Some(current) = ledger.as_mut() {
            fence_identity_conflict(
                current,
                &BTreeSet::from([task_id.to_string()]),
                now_ms,
                &reason,
            );
            store.save(current, now_ms)?;
        }
        return Ok(Some(reason));
    }
    if let Some(task_id) = bound_task.as_deref()
        && let Some(current) = ledger.as_mut()
    {
        let conflicting_binding = current
            .reservations
            .get(task_id)
            .and_then(|reservation| reservation.agent_id_hash.as_deref())
            .is_some_and(|bound_hash| {
                bound_hash != agent_hash && !is_provisional_task_binding(bound_hash, task_id)
            });
        if conflicting_binding {
            let reason = format!(
                "{AGENT_ID_COLLISION_ERROR_CODE}: child agent_id 与 attempt `{task_id}` 的既有运行时身份不一致；该 attempt 已被 fence"
            );
            fence_identity_conflict(
                current,
                &BTreeSet::from([task_id.to_string()]),
                now_ms,
                &reason,
            );
            store.save(current, now_ms)?;
            return Ok(Some(reason));
        }
        if let Some(reservation) = current.reservations.get_mut(task_id)
            && reservation.state.is_active()
            && reservation.fenced_at_ms.is_none()
            && reservation.agent_id_hash.as_deref() != Some(agent_hash.as_str())
        {
            if reservation.state == ReservationState::Pending {
                reservation.state = ReservationState::Running;
            }
            reservation.agent_id_hash = Some(agent_hash.clone());
            reservation.started_at_ms.get_or_insert(now_ms);
            reservation.updated_at_ms = now_ms;
            store.save(current, now_ms)?;
        }
    }
    let bound_reservation = bound_task.as_ref().and_then(|task_id| {
        ledger
            .as_ref()
            .and_then(|ledger| ledger.reservations.get(task_id))
    });
    let role = bound_reservation.map(|reservation| reservation.role.as_str());
    let decision = loaded_rules.rules.evaluate(&RuleContext {
        actor: RuleActor::Child,
        role,
        tool_name,
        tool_class,
    });
    let capability_denial = match tool_class {
        ToolClass::Read => match bound_reservation {
            None => Some(format!(
                "{UNBOUND_ATTEMPT_ERROR_CODE}: Codey 资源门禁：当前 child 无法通过派生回执或生命周期 transcript 与有效活动 attempt 安全关联，禁止执行读取工具。请停止本次调用并把错误码返回主代理；不要猜测 task 归属绕过身份绑定。"
            )),
            Some(reservation)
                if !reservation.state.is_active() || reservation.fenced_at_ms.is_some() =>
            {
                Some(format!(
                    "{UNBOUND_ATTEMPT_ERROR_CODE}: Codey 资源门禁：attempt `{}` 已终态、过期或被 fence，禁止继续读取。",
                    reservation.attempt_id
                ))
            }
            Some(reservation) if !reservation_declares_read(reservation) => Some(format!(
                "Codey 能力门禁：attempt `{}` 未声明 `files.read` capability，禁止读取工具 `{tool_name}`。",
                reservation.attempt_id
            )),
            Some(_) => None,
        },
        ToolClass::Command => match bound_reservation {
            None => Some(format!(
                "{UNBOUND_ATTEMPT_ERROR_CODE}: Codey 能力门禁：当前 child 没有由派生结果或生命周期事件绑定的有效 attempt，禁止执行命令。不要重试命令或等待门禁自行恢复；请立即把该错误码返回主代理，由主代理使用全新的 task_name 重新派生或直接接管。"
            )),
            Some(reservation)
                if !reservation.state.is_active() || reservation.fenced_at_ms.is_some() =>
            {
                Some(format!(
                    "{UNBOUND_ATTEMPT_ERROR_CODE}: Codey 能力门禁：attempt `{}` 已终态、过期或被 fence，禁止继续执行命令。不要重试或等待恢复；请立即把该错误码返回主代理。",
                    reservation.attempt_id
                ))
            }
            Some(reservation) if !reservation_declares_command(reservation) => Some(format!(
                "Codey 能力门禁：attempt `{}` 未声明 command.execute capability，禁止工具 `{tool_name}`。读取被拒绝时不得用 Bash 回退；应由根代理修正契约或直接接管。",
                reservation.attempt_id
            )),
            Some(reservation) if !reservation_declares_write(reservation) => Some(format!(
                "Codey 能力门禁：attempt `{}` 不是可写角色或未声明 `workspace.write` capability，禁止写入工具 `{tool_name}`。",
                reservation.attempt_id
            )),
            Some(_) => None,
        },
        ToolClass::Network => Some(format!(
            "Codey 网络门禁：子代理基线不开放网络工具 `{tool_name}`；契约 capability 不能放宽该边界。"
        )),
        ToolClass::Write => match bound_reservation {
            None => Some(format!(
                "{UNBOUND_ATTEMPT_ERROR_CODE}: Codey 能力/资源门禁：当前 child 未绑定有效 attempt，禁止执行写入工具。不要重试写入或等待门禁自行恢复；请立即把该错误码返回主代理，由主代理使用全新的 task_name 重新派生或直接接管。"
            )),
            Some(reservation)
                if !reservation.state.is_active() || reservation.fenced_at_ms.is_some() =>
            {
                Some(format!(
                    "{UNBOUND_ATTEMPT_ERROR_CODE}: Codey 能力/资源门禁：attempt `{}` 已终态、过期或被 fence，禁止恢复写权限。不要重试或等待恢复；请立即把该错误码返回主代理。",
                    reservation.attempt_id
                ))
            }
            Some(reservation) if !reservation_declares_write(reservation) => Some(format!(
                "Codey 能力门禁：attempt `{}` 不是可写角色或未声明 `workspace.write` capability，禁止写入工具 `{tool_name}`。",
                reservation.attempt_id
            )),
            Some(_) => None,
        },
        ToolClass::Collaboration if !safe_child_reporting_tool(tool_name, tool_input) => {
            Some(
                "Codey 子代理协作门禁：child 只能使用 `agents.send_message` 向 `/root` 回报；不得查看、等待、中断、追派或向其他代理发送消息。"
                    .to_string(),
            )
        }
        _ => None,
    };
    let trace = bound_reservation
        .map(reservation_trace)
        .unwrap_or_else(|| TraceContext::new(None));
    let audit_task = bound_task.as_deref().unwrap_or("unbound");
    let mut audit = SubagentTraceEvent::new(
        now_ms,
        &trace,
        TraceEventKind::RuleEvaluated,
        if decision.effect == RuleEffect::Allow && capability_denial.is_none() {
            ExecutionStatus::Running
        } else {
            ExecutionStatus::Failed
        },
        runtime_id,
        session_id,
        audit_task,
        Some(agent_id),
        role,
    );
    audit
        .attributes
        .insert("rule.id".into(), Value::String(decision.rule_id.clone()));
    audit.attributes.insert(
        "rule.priority".into(),
        Value::Number(decision.priority.into()),
    );
    audit.attributes.insert(
        "rule.effect".into(),
        Value::String(format!("{:?}", decision.effect).to_ascii_lowercase()),
    );
    audit.attributes.insert(
        "rule.conflicts".into(),
        Value::Array(
            decision
                .conflicts
                .iter()
                .cloned()
                .map(Value::String)
                .collect(),
        ),
    );
    audit.attributes.insert(
        "tool.class".into(),
        Value::String(format!("{:?}", tool_class).to_ascii_lowercase()),
    );
    audit.attributes.insert(
        "rules.revision".into(),
        Value::Number(loaded_rules.rules.revision.into()),
    );
    if let Some(reservation) = bound_reservation {
        audit.attributes.insert(
            "reservation.policy_revision".into(),
            Value::Number(reservation.policy_revision.into()),
        );
        audit.attributes.insert(
            "fencing.token".into(),
            Value::Number(reservation.fencing_token.into()),
        );
        audit.attributes.insert(
            "capabilities".into(),
            Value::Array(
                reservation
                    .capabilities
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
    }
    TraceRecorder::new(state_root).record_best_effort(&audit);
    if let Some(reason) = capability_denial {
        return Ok(Some(reason));
    }
    if decision.effect == RuleEffect::Deny {
        return Ok(Some(format!(
            "Codey 规则门禁：规则 `{}`（优先级 {}）拒绝工具 `{tool_name}`：{}",
            decision.rule_id, decision.priority, decision.explanation
        )));
    }
    Ok(None)
}

pub(crate) fn pre_root_tool(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    tool_input: Option<&Value>,
    now_ms: u64,
) -> Result<Option<String>> {
    if let Some(reason) =
        batch_decision_root_tool_reason(state_root, runtime_id, session_id, tool_input, now_ms)?
    {
        return Ok(Some(reason));
    }
    let Some(command) = extract_command(tool_input) else {
        return Ok(None);
    };
    let Some((task_id, check_id, command_body)) = parse_acceptance_marker(command) else {
        if command.trim_start().starts_with("# codey-accept:") {
            return Ok(Some(
                "Codey 机械验收门禁：验收标记格式无效；必须使用 `# codey-accept:<task_id>:<check_id>` 作为命令首行。"
                    .to_string(),
            ));
        }
        return Ok(None);
    };
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(Some("Codey 机械验收门禁：找不到对应编排账本。".to_string()));
    };
    let Some(reservation) = ledger.reservations.get(task_id) else {
        return Ok(Some(format!(
            "Codey 机械验收门禁：不存在任务 `{task_id}`。"
        )));
    };
    if !matches!(
        reservation.state,
        ReservationState::Terminal | ReservationState::Recovered
    ) {
        return Ok(Some(format!(
            "Codey 机械验收门禁：任务 `{task_id}` 尚未进入终态；必须等待子代理完成后再运行验收。"
        )));
    }
    let Some(check) = reservation
        .acceptance
        .iter()
        .find(|check| check.id == check_id)
    else {
        return Ok(Some(format!(
            "Codey 机械验收门禁：任务 `{task_id}` 不存在验收项 `{check_id}`。"
        )));
    };
    if check.status == AcceptanceStatus::Passed {
        return Ok(Some(format!(
            "Codey 机械验收门禁：任务 `{task_id}` 的验收项 `{check_id}` 已通过，无需重复执行。"
        )));
    }
    if hash_component(command_body.trim()) != check.command_hash {
        return Ok(Some(format!(
            "Codey 机械验收门禁：验收命令与任务 `{task_id}` 的契约不一致；必须逐字执行账本记录的命令。"
        )));
    }
    Ok(None)
}

pub(crate) fn post_root_tool(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    tool_input: Option<&Value>,
    tool_response: Option<&Value>,
    now_ms: u64,
) -> Result<()> {
    let Some(command) = extract_command(tool_input) else {
        return Ok(());
    };
    let Some((task_id, check_id, command_body)) = parse_acceptance_marker(command) else {
        return Ok(());
    };
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(mut ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(());
    };
    let Some(reservation) = ledger.reservations.get_mut(task_id) else {
        return Ok(());
    };
    if !matches!(
        reservation.state,
        ReservationState::Terminal | ReservationState::Recovered
    ) {
        return Ok(());
    }
    let Some(check) = reservation
        .acceptance
        .iter_mut()
        .find(|check| check.id == check_id)
    else {
        return Ok(());
    };
    if hash_component(command_body.trim()) != check.command_hash {
        return Ok(());
    }
    check.attempted_at_ms = Some(now_ms);
    check.evidence_hash = tool_response.map(canonical_value_hash);
    check.blocked_stop_count = 0;
    check.blocked_since_ms = Some(now_ms);
    check.release_notice_delivered_at_ms = None;
    match classify_acceptance_evidence(tool_response) {
        AcceptanceEvidence::Passed => {
            check.status = AcceptanceStatus::Passed;
            check.release_reason = None;
        }
        evidence => {
            check.failure_count = check.failure_count.saturating_add(1);
            check.release_reason = Some(evidence.failure_reason().to_string());
            check.status = if check.failure_count >= MAX_ACCEPTANCE_FAILURES {
                AcceptanceStatus::Unverifiable
            } else {
                AcceptanceStatus::Failed
            };
        }
    }
    store.save(&mut ledger, now_ms)
}

pub(crate) fn pending_acceptance_reason(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    now_ms: u64,
) -> Result<Option<String>> {
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(mut ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(None);
    };
    let mut commands = Vec::new();
    let mut release_notices = Vec::new();
    for reservation in ledger.reservations.values_mut() {
        if reservation.write_capable && !reservation.spawn_failed {
            if !matches!(
                reservation.state,
                ReservationState::Terminal | ReservationState::Recovered
            ) {
                reservation.state = ReservationState::Terminal;
                reservation.outcome = ExecutionOutcome::Lost;
                reservation.agent_id_hash = None;
                reservation.updated_at_ms = now_ms;
                reservation.completed_at_ms = Some(now_ms);
                reservation.fenced_at_ms = Some(now_ms);
                reservation.error_message = Some(
                    "root turn settled without an authoritative successful child outcome"
                        .to_string(),
                );
            }
            for check in &mut reservation.acceptance {
                if check.status == AcceptanceStatus::Passed {
                    continue;
                }
                if check.status == AcceptanceStatus::Unverifiable {
                    if check.release_notice_delivered_at_ms.is_none() {
                        check.release_notice_delivered_at_ms = Some(now_ms);
                        release_notices.push(format!(
                            "- `{}:{}`：{}（失败 {} 次）",
                            reservation.task_id,
                            check.id,
                            check
                                .release_reason
                                .as_deref()
                                .unwrap_or("无法取得可信的验收证据"),
                            check.failure_count
                        ));
                    }
                    continue;
                }

                let blocked_since_ms = *check.blocked_since_ms.get_or_insert(now_ms);
                check.blocked_stop_count = check.blocked_stop_count.saturating_add(1);
                let stalled = check.blocked_stop_count >= MAX_UNCHANGED_ACCEPTANCE_STOPS
                    || now_ms.saturating_sub(blocked_since_ms) >= ACCEPTANCE_STALL_GRACE_MILLIS;
                if stalled {
                    check.status = AcceptanceStatus::Unverifiable;
                    check.release_reason = Some(
                        if check.blocked_stop_count >= MAX_UNCHANGED_ACCEPTANCE_STOPS {
                            format!(
                                "验收债连续 {} 次 Stop 未取得新证据",
                                check.blocked_stop_count
                            )
                        } else {
                            "验收债持续 10 分钟未取得新证据".to_string()
                        },
                    );
                    check.release_notice_delivered_at_ms = Some(now_ms);
                    release_notices.push(format!(
                        "- `{}:{}`：{}（失败 {} 次）",
                        reservation.task_id,
                        check.id,
                        check.release_reason.as_deref().unwrap_or_default(),
                        check.failure_count
                    ));
                    continue;
                }

                commands.push(format!(
                    "# codey-accept:{}:{}\n{}",
                    reservation.task_id, check.id, check.command
                ));
            }
        }
    }
    if commands.is_empty() && release_notices.is_empty() {
        return Ok(None);
    }
    store.save(&mut ledger, now_ms)?;
    let mut sections = Vec::new();
    if !release_notices.is_empty() {
        sections.push(format!(
            "以下 {} 项验收已经达到失败或停滞上限，没有被标记为通过。门禁将在本次提示后释放这些项目；主代理必须停止自动重试，并在最终答复中明确报告未完成的验收及原因：\n{}",
            release_notices.len(),
            release_notices.join("\n")
        ));
    }
    if !commands.is_empty() {
        sections.push(format!(
            "写入型子代理还有 {} 项可继续清偿的验收债。主代理必须逐项原样执行下列命令，并由可信的退出状态 `0` 清偿；子代理自报通过或改写后的命令不计入验收：\n\n{}",
            commands.len(),
            commands.join("\n\n")
        ));
    }
    Ok(Some(format!(
        "Codey 机械验收门禁：{}",
        sections.join("\n\n")
    )))
}

pub(crate) fn settle_turn(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    now_ms: u64,
) -> Result<()> {
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(());
    };
    anyhow::ensure!(
        !ledger
            .reservations
            .values()
            .any(reservation_has_pending_acceptance),
        "Codey 子代理机械验收债尚未清偿"
    );
    anyhow::ensure!(
        !ledger.decision_required
            || !current_batch_has_admitted_agent(&ledger)
            || matches!(
                ledger.batch_decision,
                BatchDecisionState::Committed {
                    decision: RootBatchDecision::Complete | RootBatchDecision::Blocked,
                    ..
                } | BatchDecisionState::ControlPlaneFailed { .. }
            ),
        "Codey 子代理批次尚未提交 complete/blocked 终局决策"
    );
    anyhow::ensure!(
        !ledger_has_unverifiable_acceptance(&ledger)
            || matches!(
                ledger.batch_decision,
                BatchDecisionState::Committed {
                    decision: RootBatchDecision::Blocked,
                    ..
                } | BatchDecisionState::ControlPlaneFailed { .. }
            ),
        "Codey 子代理存在无法验证的机械验收，终局决策必须为 blocked"
    );
    store.write_settlement_receipt(&ledger, runtime_id, session_id)?;
    store.remove()
}

pub(crate) fn end_session(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    now_ms: u64,
) -> Result<()> {
    LedgerStore::open(state_root, session_id)?
        .remove_for_session_end(runtime_id, session_id, now_ms)
}

fn ledger_has_outstanding(ledger: &SessionLedger) -> bool {
    ledger.reservations.values().any(|reservation| {
        !reservation.state.is_settled()
            || reservation_has_pending_acceptance(reservation)
            || reservation
                .acceptance
                .iter()
                .any(|check| check.status == AcceptanceStatus::Unverifiable)
    }) || matches!(
        ledger.batch_decision,
        BatchDecisionState::Awaiting { .. }
            | BatchDecisionState::Pending { .. }
            | BatchDecisionState::Committed {
                decision: RootBatchDecision::SpawnNextBatch | RootBatchDecision::ContinueRoot,
                ..
            }
    )
}

fn ledger_has_unverifiable_acceptance(ledger: &SessionLedger) -> bool {
    ledger.reservations.values().any(|reservation| {
        reservation
            .acceptance
            .iter()
            .any(|check| check.status == AcceptanceStatus::Unverifiable)
    })
}

fn reservation_declares_command(reservation: &Reservation) -> bool {
    reservation
        .capabilities
        .iter()
        .any(|capability| capability == "command.execute")
}

fn reservation_declares_read(reservation: &Reservation) -> bool {
    reservation
        .capabilities
        .iter()
        .any(|capability| capability == "files.read")
}

fn reservation_declares_write(reservation: &Reservation) -> bool {
    reservation.write_capable
        && reservation
            .capabilities
            .iter()
            .any(|capability| capability == "workspace.write")
}

fn reservation_trace(reservation: &Reservation) -> TraceContext {
    TraceContext {
        trace_id: if reservation.trace_id.is_empty() {
            hash_component(&reservation.task_id)
        } else {
            reservation.trace_id.clone()
        },
        parent_id: reservation.parent_id.clone(),
    }
}

#[cfg(test)]
fn prepare_contract(tool_input: Option<&Value>) -> std::result::Result<PreparedContract, String> {
    prepare_contract_with_workspace(tool_input, None)
}

#[cfg(test)]
fn prepare_contract_with_workspace(
    tool_input: Option<&Value>,
    hook_workspace_root: Option<&str>,
) -> std::result::Result<PreparedContract, String> {
    prepare_contract_with_rules(tool_input, hook_workspace_root, rules::embedded())
}

fn prepare_contract_with_rules(
    tool_input: Option<&Value>,
    hook_workspace_root: Option<&str>,
    rule_set: &RuleSet,
) -> std::result::Result<PreparedContract, String> {
    let input = tool_input
        .and_then(Value::as_object)
        .ok_or_else(|| contract_error("spawn 输入不是 JSON object"))?;
    let task_name = string_field(input, &["task_name", "taskName"])
        .ok_or_else(|| contract_error("缺少 task_name"))?;
    let role = string_field(
        input,
        &["agent_type", "agentType", "agent_role", "agentRole"],
    )
    .ok_or_else(|| contract_error("缺少 agent_type"))?;
    let message = string_field(input, &["message", "prompt"])
        .ok_or_else(|| contract_error("缺少 message"))?;
    if string_field(input, &["fork_turns", "forkTurns"]).unwrap_or("none") != "none" {
        return Err(contract_error("fork_turns 必须为 none"));
    }
    let policy = rule_set
        .role_policy(role)
        .ok_or_else(|| contract_error(&format!("未知或不允许的 agent_type `{role}`")))?;
    if is_opaque_encrypted_message(message) {
        return prepare_opaque_contract(task_name, role, policy, hook_workspace_root);
    }
    let line = message
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .ok_or_else(|| contract_error("message 为空"))?;
    let (payload, legacy_v1) = if let Some(payload) = line.strip_prefix(CONTRACT_PREFIX) {
        (payload, false)
    } else if let Some(payload) = line.strip_prefix(LEGACY_CONTRACT_PREFIX_V1) {
        (payload, true)
    } else {
        return Err(contract_error(
            "message 最后一行缺少 CODEY_DELEGATION_V2 契约",
        ));
    };
    if payload.chars().count() > MAX_CONTRACT_LINE_CHARS {
        return Err(contract_error("契约行超过 8K 字符"));
    }
    let mut contract_value: Value = serde_json::from_str(payload)
        .map_err(|error| contract_error(&format!("契约 JSON 无效：{error}")))?;
    if legacy_v1 {
        let values = contract_value
            .as_object_mut()
            .ok_or_else(|| contract_error("V1 契约 JSON 必须是 object"))?;
        for retired in [
            "calls",
            "files",
            "dirs",
            "large",
            "risk",
            "budget_class",
            "branch_calls",
        ] {
            values.remove(retired);
        }
    }
    let contract: DelegationContract = serde_json::from_value(contract_value)
        .map_err(|error| contract_error(&format!("契约 JSON 无效：{error}")))?;
    validate_task_id(&contract.id)?;
    if contract.id != task_name {
        return Err(contract_error("契约 id 必须与 task_name 完全一致"));
    }
    validate_delegation_reason(&contract.reason)?;
    if contract.visual != policy.visual {
        return Err(contract_error(if policy.visual {
            "视觉角色的契约必须设置 visual=true"
        } else {
            "非视觉角色不能声明 visual=true"
        }));
    }
    if contract.read_paths.len() > MAX_CLAIMS_PER_MODE
        || contract.write_paths.len() > MAX_CLAIMS_PER_MODE
    {
        return Err(contract_error("read/write 资源声明各自最多 16 项"));
    }
    if contract.acceptance.len() > MAX_ACCEPTANCE_CHECKS {
        return Err(contract_error("checks 最多 8 项"));
    }
    if contract.capabilities.len() > 16 {
        return Err(contract_error("capabilities 最多 16 项"));
    }
    let mut capabilities = contract
        .capabilities
        .iter()
        .map(|capability| capability.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();
    for capability in &capabilities {
        if capability.is_empty()
            || capability.chars().count() > 64
            || !capability.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
            })
        {
            return Err(contract_error(
                "capabilities 必须为 1..=64 个 ASCII 字母、数字、点、连字符或下划线",
            ));
        }
        if !matches!(
            capability.as_str(),
            "files.read" | "workspace.write" | "command.execute"
        ) {
            return Err(contract_error(&format!(
                "未知 capability `{capability}`；仅支持 files.read、workspace.write、command.execute"
            )));
        }
    }
    capabilities.sort();
    capabilities.dedup();
    if legacy_v1 && capabilities.is_empty() && policy.access == RoleAccess::ReadOnly {
        capabilities.push("files.read".to_string());
    }
    if contract.deadline_ms.is_some_and(|deadline| deadline == 0) {
        return Err(contract_error("deadline_ms 必须大于 0"));
    }
    for (name, schema) in [
        ("input_schema", contract.input_schema.as_ref()),
        ("output_schema", contract.output_schema.as_ref()),
    ] {
        if let Some(schema) = schema {
            validate_contract_schema(name, schema)?;
        }
    }
    let trace =
        TraceContext::normalized(contract.trace_id.as_deref(), contract.parent_id.as_deref())
            .map_err(|error| contract_error(&error))?;
    let workspace_root = if let Some(root) = contract.workspace_root.as_deref() {
        Some(
            normalize_coordination_path(root)
                .map_err(|error| contract_error(&format!("root 无效：{error}")))?,
        )
    } else {
        hook_workspace_root.and_then(|root| normalize_coordination_path(root).ok())
    };
    let mut read_paths = normalize_claims(&contract.read_paths, workspace_root.as_deref())?;
    let write_paths = normalize_claims(&contract.write_paths, workspace_root.as_deref())?;
    if read_paths.is_empty() {
        if policy.access == RoleAccess::Write {
            read_paths.clone_from(&write_paths);
        } else if let Some(root) = workspace_root.as_ref() {
            read_paths.push(root.clone());
        }
    }
    match policy.access {
        RoleAccess::ReadOnly => {
            if !write_paths.is_empty() || !contract.acceptance.is_empty() {
                return Err(contract_error("只读角色不能声明 write 或 checks"));
            }
            if capabilities.iter().any(|capability| {
                matches!(capability.as_str(), "workspace.write" | "command.execute")
            }) {
                return Err(contract_error(
                    "只读角色不能声明 workspace.write 或 command.execute capability",
                ));
            }
        }
        RoleAccess::Write => {
            if write_paths.is_empty() {
                return Err(contract_error("写入角色必须声明至少一个 write ownership"));
            }
            if contract.acceptance.is_empty() {
                return Err(contract_error("写入角色必须声明至少一个机械 checks"));
            }
            if !capabilities
                .iter()
                .any(|capability| capability == "workspace.write")
            {
                return Err(contract_error(
                    "写入角色必须声明 workspace.write capability",
                ));
            }
        }
    }
    if !capabilities
        .iter()
        .any(|capability| capability == "files.read")
    {
        return Err(contract_error(
            "所有可执行契约都必须显式声明 files.read capability",
        ));
    }
    let mut check_ids = BTreeSet::new();
    let mut total_check_chars = 0_usize;
    for check in &contract.acceptance {
        validate_task_id(&check.id)?;
        if !check_ids.insert(check.id.as_str()) {
            return Err(contract_error("checks id 不能重复"));
        }
        let command = check.command.trim();
        if command.is_empty() || command.chars().count() > MAX_ACCEPTANCE_COMMAND_CHARS {
            return Err(contract_error("checks cmd 必须为 1..=1024 个字符"));
        }
        total_check_chars = total_check_chars.saturating_add(command.chars().count());
        if total_check_chars > MAX_ACCEPTANCE_TOTAL_CHARS {
            return Err(contract_error("checks 命令总长度不能超过 4096 个字符"));
        }
        if command
            .lines()
            .next()
            .is_some_and(|line| line.trim_start().starts_with("# codey-accept:"))
        {
            return Err(contract_error("checks cmd 不能自行包含 codey-accept 标记"));
        }
    }
    Ok(PreparedContract {
        invocation_mode: contract.mode,
        capabilities,
        trace,
        contract,
        role: role.to_string(),
        policy,
        workspace_root,
        read_paths,
        native_read_scope: false,
        write_paths,
    })
}

fn is_opaque_encrypted_message(message: &str) -> bool {
    let message = message.trim();
    message.len() >= 128
        && message.starts_with("gAAAAA")
        && message
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'='))
}

fn prepare_opaque_contract(
    task_name: &str,
    role: &str,
    policy: RolePolicy,
    hook_workspace_root: Option<&str>,
) -> std::result::Result<PreparedContract, String> {
    validate_task_id(task_name)?;
    if policy.access == RoleAccess::Write {
        return Err(contract_error(
            "message 已由上游加密，无法验证 write ownership 与机械 checks；写入角色必须使用可验证的明文或签名 sidecar 契约",
        ));
    }
    let workspace_root =
        hook_workspace_root.and_then(|root| normalize_coordination_path(root).ok());
    let workspace_claims = workspace_root.iter().cloned().collect::<Vec<_>>();
    let read_paths = workspace_claims;
    let write_paths = Vec::new();
    let contract = DelegationContract {
        id: task_name.to_string(),
        reason: "encrypted_message".to_string(),
        visual: policy.visual,
        workspace_root: workspace_root.clone(),
        read_paths: read_paths.clone(),
        write_paths: write_paths.clone(),
        acceptance: Vec::new(),
        mode: InvocationMode::Async,
        trace_id: None,
        parent_id: None,
        capabilities: vec!["files.read".to_string()],
        deadline_ms: None,
        input_schema: None,
        output_schema: None,
    };
    Ok(PreparedContract {
        trace: TraceContext::new(None),
        invocation_mode: InvocationMode::Async,
        capabilities: contract.capabilities.clone(),
        contract,
        role: role.to_string(),
        policy,
        workspace_root,
        read_paths,
        native_read_scope: true,
        write_paths,
    })
}

fn contract_error(detail: &str) -> String {
    format!(
        "Codey 自适应委派门禁：{detail}。请在 message 最后一行追加紧凑契约，例如：{CONTRACT_PREFIX}{{\"id\":\"scan_auth\",\"why\":\"breadth\",\"visual\":false,\"read\":[],\"write\":[],\"capabilities\":[\"files.read\"],\"checks\":[]}}"
    )
}

fn validate_task_id(value: &str) -> std::result::Result<(), String> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(contract_error(
            "id/check id 只允许 1..=64 个小写字母、数字或下划线",
        ));
    }
    Ok(())
}

fn validate_delegation_reason(reason: &str) -> std::result::Result<(), String> {
    let reason = reason.trim();
    if reason.is_empty()
        || reason.chars().count() > MAX_REASON_CHARS
        || reason.chars().any(char::is_control)
    {
        return Err(contract_error(
            "why 必须为 1..=128 个不含换行或控制字符的审计说明",
        ));
    }
    Ok(())
}

fn validate_contract_schema(name: &str, schema: &Value) -> std::result::Result<(), String> {
    if !schema.is_object() {
        return Err(contract_error(&format!("{name} 必须为 JSON object")));
    }
    let encoded = serde_json::to_vec(schema)
        .map_err(|error| contract_error(&format!("{name} 无法序列化：{error}")))?;
    if encoded.len() > MAX_SCHEMA_BYTES {
        return Err(contract_error(&format!(
            "{name} 序列化后不能超过 {MAX_SCHEMA_BYTES} 字节"
        )));
    }
    validate_schema_node(name, schema, 0)
}

fn validate_schema_node(
    name: &str,
    schema: &Value,
    depth: usize,
) -> std::result::Result<(), String> {
    if depth > MAX_SCHEMA_DEPTH {
        return Err(contract_error(&format!(
            "{name} 嵌套深度不能超过 {MAX_SCHEMA_DEPTH}"
        )));
    }
    if schema.is_boolean() {
        return Ok(());
    }
    let Some(object) = schema.as_object() else {
        return Err(contract_error(&format!(
            "{name} 的子 schema 必须为 object 或 boolean"
        )));
    };
    if let Some(schema_type) = object.get("type") {
        let valid_type = |value: &str| {
            matches!(
                value,
                "null" | "boolean" | "object" | "array" | "number" | "integer" | "string"
            )
        };
        let valid = schema_type.as_str().is_some_and(valid_type)
            || schema_type.as_array().is_some_and(|values| {
                !values.is_empty()
                    && values
                        .iter()
                        .all(|value| value.as_str().is_some_and(valid_type))
            });
        if !valid {
            return Err(contract_error(&format!(
                "{name}.type 必须是合法 JSON Schema 类型或非空类型数组"
            )));
        }
    }
    if let Some(required) = object.get("required")
        && !required.as_array().is_some_and(|values| {
            values
                .iter()
                .all(|value| value.as_str().is_some_and(|value| !value.is_empty()))
        })
    {
        return Err(contract_error(&format!(
            "{name}.required 必须为非空字符串数组"
        )));
    }
    if let Some(properties) = object.get("properties") {
        let Some(properties) = properties.as_object() else {
            return Err(contract_error(&format!(
                "{name}.properties 必须为 JSON object"
            )));
        };
        for (property, child) in properties {
            validate_schema_node(&format!("{name}.properties.{property}"), child, depth + 1)?;
        }
    }
    if let Some(items) = object.get("items") {
        match items {
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    validate_schema_node(&format!("{name}.items[{index}]"), child, depth + 1)?;
                }
            }
            child => validate_schema_node(&format!("{name}.items"), child, depth + 1)?,
        }
    }
    for keyword in ["allOf", "anyOf", "oneOf"] {
        if let Some(children) = object.get(keyword) {
            let Some(children) = children.as_array() else {
                return Err(contract_error(&format!("{name}.{keyword} 必须为数组")));
            };
            for (index, child) in children.iter().enumerate() {
                validate_schema_node(&format!("{name}.{keyword}[{index}]"), child, depth + 1)?;
            }
        }
    }
    for keyword in ["not", "additionalProperties", "contains"] {
        if let Some(child) = object.get(keyword) {
            validate_schema_node(&format!("{name}.{keyword}"), child, depth + 1)?;
        }
    }
    Ok(())
}

fn normalize_claims(
    claims: &[String],
    workspace_root: Option<&str>,
) -> std::result::Result<Vec<String>, String> {
    let mut normalized = BTreeSet::new();
    for claim in claims {
        let path = if is_absolute_path(claim) {
            normalize_coordination_path(claim)
        } else if let Some(root) = workspace_root {
            normalize_coordination_path(&format!("{}/{}", root.trim_end_matches('/'), claim))
        } else {
            Err("相对资源路径需要绝对 root".to_string())
        }
        .map_err(|error| contract_error(&format!("资源路径 `{claim}` 无效：{error}")))?;
        normalized.insert(path);
    }
    Ok(normalized.into_iter().collect())
}

/// Canonicalize existing ancestors when possible so coordination claims for
/// obvious aliases overlap. These paths are scheduling metadata, not a file
/// ACL: metadata/canonicalization failures fall back to the lexical absolute
/// path, while the Codex executor remains the only filesystem authority.
fn normalize_coordination_path(value: &str) -> std::result::Result<String, String> {
    let lexical = normalize_absolute_path(value)?;
    let path = PathBuf::from(&lexical);
    if !path.is_absolute() {
        // A foreign-platform drive path cannot be resolved on this host. Keep
        // its already-normalized lexical form for portable ledger migration.
        return Ok(lexical);
    }
    let mut ancestor = path.as_path();
    let mut missing = Vec::new();
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = ancestor.file_name() else {
                    return Ok(lexical);
                };
                missing.push(name.to_os_string());
                let Some(parent) = ancestor.parent() else {
                    return Ok(lexical);
                };
                ancestor = parent;
            }
            Err(_) => return Ok(lexical),
        }
    }
    let Ok(mut resolved) = fs::canonicalize(ancestor) else {
        return Ok(lexical);
    };
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(normalize_absolute_path(&resolved.to_string_lossy()).unwrap_or(lexical))
}

fn normalize_absolute_path(value: &str) -> std::result::Result<String, String> {
    let mut replaced = value.trim().replace('\\', "/");
    if let Some(verbatim) = replaced.strip_prefix("//?/") {
        replaced = if verbatim
            .get(..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("unc/"))
        {
            format!("//{}", &verbatim[4..])
        } else {
            verbatim.to_string()
        };
    }
    if replaced.is_empty() || replaced.contains(['*', '?', '[', ']']) {
        return Err("必须是无 glob 的绝对路径".to_string());
    }
    let (prefix, rest) = if let Some(rest) = replaced.strip_prefix("//") {
        ("//".to_string(), rest)
    } else if replaced.starts_with('/') {
        ("/".to_string(), replaced.trim_start_matches('/'))
    } else if replaced.len() >= 3
        && replaced.as_bytes()[0].is_ascii_alphabetic()
        && replaced.as_bytes()[1] == b':'
        && replaced.as_bytes()[2] == b'/'
    {
        (
            format!(
                "{}:/",
                (replaced.as_bytes()[0] as char).to_ascii_uppercase()
            ),
            &replaced[3..],
        )
    } else {
        return Err("必须是 Unix、UNC 或盘符绝对路径".to_string());
    };
    let mut components = Vec::new();
    for component in rest.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.pop().is_none() {
                    return Err("路径不能越过根目录".to_string());
                }
            }
            component => components.push(component),
        }
    }
    let joined = components.join("/");
    let mut result = if joined.is_empty() {
        prefix
    } else {
        format!("{prefix}{joined}")
    };
    if cfg!(windows) {
        result.make_ascii_lowercase();
    }
    Ok(result)
}

fn is_absolute_path(value: &str) -> bool {
    let value = value.trim().replace('\\', "/");
    value.starts_with('/')
        || value.starts_with("//")
        || (value.len() >= 3
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value.as_bytes()[1] == b':'
            && value.as_bytes()[2] == b'/')
}

fn resource_conflict(prepared: &PreparedContract, ledger: &SessionLedger) -> Option<String> {
    for existing in ledger.reservations.values().filter(|reservation| {
        !reservation.spawn_failed
            && (reservation.state != ReservationState::Terminal
                || reservation_has_pending_acceptance(reservation))
    }) {
        if prepared.native_read_scope && !existing.write_paths.is_empty() {
            return Some(format!(
                "Codey 能力/资源冲突门禁：密文只读任务 `{}` 的具体 read scope 对 Hook 不可见，不能与活动写任务 `{}` 并行；请等待写任务结束后再派发。",
                prepared.contract.id, existing.task_id
            ));
        }
        if existing.native_read_scope && !prepared.write_paths.is_empty() {
            return Some(format!(
                "Codey 能力/资源冲突门禁：写任务 `{}` 不能与活动密文只读任务 `{}` 并行，因为后者的具体 read scope 对 Hook 不可见；请先等待只读任务结束。",
                prepared.contract.id, existing.task_id
            ));
        }
        for new_write in &prepared.write_paths {
            if let Some(existing_path) = existing
                .write_paths
                .iter()
                .chain(existing.read_paths.iter())
                .find(|existing_path| paths_overlap(new_write, existing_path))
            {
                return Some(format!(
                    "Codey 能力/资源冲突门禁：任务 `{}` 的写入 `{new_write}` 与活动任务 `{}` 的资源 `{existing_path}` 重叠；请合并任务、改为串行或声明互斥 ownership。",
                    prepared.contract.id, existing.task_id
                ));
            }
        }
        for new_read in &prepared.read_paths {
            if let Some(existing_write) = existing
                .write_paths
                .iter()
                .find(|existing_write| paths_overlap(new_read, existing_write))
            {
                return Some(format!(
                    "Codey 能力/资源冲突门禁：任务 `{}` 的读取 `{new_read}` 与活动写任务 `{}` 的 `{existing_write}` 重叠；为避免读取过期状态，请改为串行。",
                    prepared.contract.id, existing.task_id
                ));
            }
        }
    }
    None
}

fn paths_overlap(left: &str, right: &str) -> bool {
    path_is_within(left, right) || path_is_within(right, left)
}

fn path_is_within(path: &str, parent: &str) -> bool {
    path == parent
        || parent.ends_with('/') && path.starts_with(parent)
        || path
            .strip_prefix(parent)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

pub(crate) fn safe_child_reporting_tool(tool_name: &str, tool_input: Option<&Value>) -> bool {
    if rules::normalize_tool_name(tool_name) != "send_message" {
        return false;
    }
    let target = tool_input.and_then(Value::as_object).and_then(|values| {
        values.iter().find_map(|(key, value)| {
            (normalized_identifier(key) == "target")
                .then(|| value.as_str().map(str::trim))
                .flatten()
        })
    });
    target.is_some_and(|target| matches!(target.trim_end_matches('/'), "root" | "/root"))
}

fn extract_command(value: Option<&Value>) -> Option<&str> {
    let value = value?;
    match value {
        Value::String(command) => Some(command),
        Value::Object(values) => {
            for key in ["command", "cmd"] {
                if let Some(command) = values.get(key).and_then(Value::as_str) {
                    return Some(command);
                }
            }
            None
        }
        _ => None,
    }
}

fn parse_acceptance_marker(command: &str) -> Option<(&str, &str, &str)> {
    let mut lines = command.lines();
    let first = lines.next()?.trim();
    let marker = first.strip_prefix("# codey-accept:")?;
    let (task_id, check_id) = marker.split_once(':')?;
    if task_id.is_empty() || check_id.is_empty() || check_id.contains(':') {
        return None;
    }
    let body_offset = command.find('\n').map_or(command.len(), |index| index + 1);
    Some((task_id, check_id, &command[body_offset..]))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AcceptanceEvidence {
    Passed,
    CommandFailed,
    MissingExitStatus,
}

impl AcceptanceEvidence {
    fn failure_reason(self) -> &'static str {
        match self {
            Self::Passed => "验收已通过",
            Self::CommandFailed => "验收命令返回失败状态",
            Self::MissingExitStatus => "上游工具响应缺少可识别的退出状态",
        }
    }
}

fn classify_acceptance_evidence(value: Option<&Value>) -> AcceptanceEvidence {
    let Some(value) = value else {
        return AcceptanceEvidence::MissingExitStatus;
    };
    let mut exit_codes = Vec::new();
    let mut error = false;
    collect_exit_status(value, &mut exit_codes, &mut error, 0, true);
    if error || exit_codes.iter().any(|exit_code| *exit_code != 0) {
        AcceptanceEvidence::CommandFailed
    } else if exit_codes.is_empty() {
        AcceptanceEvidence::MissingExitStatus
    } else {
        AcceptanceEvidence::Passed
    }
}

fn collect_exit_status(
    value: &Value,
    exit_codes: &mut Vec<i64>,
    error: &mut bool,
    depth: usize,
    allow_plain_text_status: bool,
) {
    if depth > 12 {
        return;
    }
    match value {
        Value::Array(values) => {
            for value in values {
                collect_exit_status(value, exit_codes, error, depth + 1, false);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                let key = normalized_identifier(key);
                if key == "exitcode" {
                    if let Some(code) = value
                        .as_i64()
                        .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()))
                    {
                        exit_codes.push(code);
                    }
                } else if (key == "iserror" && value.as_bool() == Some(true))
                    || (key == "error" && protocol::value_reports_nonempty_error(value))
                {
                    *error = true;
                }
                collect_exit_status(value, exit_codes, error, depth + 1, false);
            }
        }
        Value::String(value) if allow_plain_text_status && value.len() <= 64 * 1024 => {
            if let Ok(parsed) = serde_json::from_str::<Value>(value) {
                collect_exit_status(&parsed, exit_codes, error, depth + 1, false);
            } else if let Some(exit_code) = parse_plain_text_exit_code(value) {
                exit_codes.push(exit_code);
            }
        }
        _ => {}
    }
}

fn parse_plain_text_exit_code(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.len() > 1024 {
        return None;
    }
    let first_line = trimmed.lines().next()?.trim().to_ascii_lowercase();
    for prefix in [
        "exit_code",
        "exit code",
        "exitcode",
        "process exited with code",
        "command exited with code",
        "script exited with code",
        "command finished with exit code",
    ] {
        if let Some(remainder) = first_line.strip_prefix(prefix) {
            let token = remainder
                .trim_start_matches(|character: char| {
                    character.is_ascii_whitespace() || matches!(character, ':' | '=')
                })
                .split_whitespace()
                .next()?;
            let token = token.trim_end_matches(|character: char| !character.is_ascii_digit());
            return token.parse::<i64>().ok();
        }
    }
    None
}

fn response_is_explicit_failure(value: &Value) -> bool {
    protocol::response_is_explicit_spawn_failure(value)
        || parse_json_encoded_spawn_response(value)
            .as_ref()
            .is_some_and(protocol::response_is_explicit_spawn_failure)
}

fn extract_agent_identifier(value: &Value) -> Option<&str> {
    protocol::extract_agent_identifier(value)
}

/// Extracts an identity that can be bound to the reservation created by the
/// current spawn call.
///
/// Newer collaboration providers return an explicit agent identifier. Some
/// providers only return the canonical task path (for example
/// `/root/review_auth`). The latter is safe to use here only because PostToolUse
/// also carries the exact input of this spawn call: the returned task path must
/// equal that task id or `/root/<task_id>`, and lookup never descends into
/// arbitrary task output fields.
fn extract_spawn_binding_identifier(response: &Value, expected_task_id: &str) -> Option<String> {
    if let Some(identifier) = extract_agent_identifier(response) {
        return Some(identifier.to_string());
    }
    if let Some(task_name) = extract_matching_spawn_task_name(response, expected_task_id, true) {
        return Some(task_name.to_string());
    }
    // Direct collaboration tools currently expose their structured result to
    // PostToolUse as one JSON-encoded string. Parse only the complete, bounded
    // response and then apply the same provider-envelope checks; never scan
    // arbitrary prose or embedded output fragments for identity fields.
    let parsed = parse_json_encoded_spawn_response(response)?;
    extract_agent_identifier(&parsed)
        .or_else(|| extract_matching_spawn_task_name(&parsed, expected_task_id, true))
        .map(ToOwned::to_owned)
}

fn parse_json_encoded_spawn_response(response: &Value) -> Option<Value> {
    let encoded = response.as_str()?.trim();
    if encoded.is_empty() || encoded.len() > MAX_SPAWN_RESPONSE_JSON_BYTES {
        return None;
    }
    serde_json::from_str(encoded).ok()
}

fn extract_matching_spawn_task_name<'a>(
    value: &'a Value,
    expected_task_id: &str,
    provider_owned: bool,
) -> Option<&'a str> {
    match value {
        Value::Object(values) => {
            if provider_owned
                && let Some(task_name) = values.iter().find_map(|(key, value)| {
                    (normalized_identifier(key) == "taskname")
                        .then(|| value.as_str().map(str::trim))
                        .flatten()
                        .filter(|value| !value.is_empty())
                })
                && spawn_task_name_matches(task_name, expected_task_id)
            {
                return Some(task_name);
            }
            values.iter().find_map(|(key, value)| {
                protocol::is_provider_envelope_field(key)
                    .then(|| extract_matching_spawn_task_name(value, expected_task_id, true))
                    .flatten()
            })
        }
        Value::Array(values) if provider_owned => values
            .iter()
            .find_map(|value| extract_matching_spawn_task_name(value, expected_task_id, true)),
        _ => None,
    }
}

fn spawn_task_name_matches(task_name: &str, expected_task_id: &str) -> bool {
    task_name == expected_task_id || task_name == format!("/root/{expected_task_id}")
}

fn collect_terminal_task_outcomes(
    value: &Value,
    ledger: &mut SessionLedger,
    terminal_tasks: &mut BTreeMap<String, ExecutionOutcome>,
    now_ms: u64,
) -> Result<()> {
    let mut observations = Vec::new();
    protocol::collect_terminal_observations(value, &mut observations);
    for observation in observations {
        let candidates = identity_task_candidates(ledger, &observation.identifier);
        if candidates.len() > 1 {
            let reason = format!(
                "终态标识 `{}` 同时指向多个 attempt（{}）",
                observation.identifier,
                candidates.iter().cloned().collect::<Vec<_>>().join(", ")
            );
            fence_identity_conflict(ledger, &candidates, now_ms, &reason);
            anyhow::bail!("{AGENT_ID_COLLISION_ERROR_CODE}: {reason}");
        }
        let Some(task_id) = candidates.into_iter().next() else {
            continue;
        };
        let outcome = match observation.outcome {
            protocol::TerminalOutcome::Succeeded => ExecutionOutcome::Succeeded,
            protocol::TerminalOutcome::Failed => ExecutionOutcome::Failed,
            protocol::TerminalOutcome::Lost => ExecutionOutcome::Lost,
        };
        terminal_tasks
            .entry(task_id)
            .and_modify(|current| *current = stricter_outcome(*current, outcome))
            .or_insert(outcome);
    }
    Ok(())
}

fn stricter_outcome(left: ExecutionOutcome, right: ExecutionOutcome) -> ExecutionOutcome {
    if matches!(left, ExecutionOutcome::Failed | ExecutionOutcome::TimedOut)
        || matches!(right, ExecutionOutcome::Failed | ExecutionOutcome::TimedOut)
    {
        if left == ExecutionOutcome::TimedOut || right == ExecutionOutcome::TimedOut {
            ExecutionOutcome::TimedOut
        } else {
            ExecutionOutcome::Failed
        }
    } else if left == ExecutionOutcome::Lost || right == ExecutionOutcome::Lost {
        ExecutionOutcome::Lost
    } else if left == ExecutionOutcome::Succeeded || right == ExecutionOutcome::Succeeded {
        ExecutionOutcome::Succeeded
    } else {
        ExecutionOutcome::Unknown
    }
}

fn spawn_task_id(tool_input: Option<&Value>) -> Option<&str> {
    let input = tool_input?.as_object()?;
    string_field(input, &["task_name", "taskName"])
}

fn followup_task_target(tool_input: Option<&Value>) -> Option<&str> {
    let input = tool_input?.as_object()?;
    string_field(input, &["target"])
        .map(str::trim)
        .filter(|target| !target.is_empty())
}

fn interrupt_task_target(tool_input: Option<&Value>) -> Option<String> {
    match tool_input? {
        Value::Object(input) => string_field(input, &["target"])
            .map(str::trim)
            .filter(|target| !target.is_empty())
            .map(ToOwned::to_owned),
        Value::String(encoded) if encoded.len() <= MAX_SPAWN_RESPONSE_JSON_BYTES => {
            let decoded = serde_json::from_str::<Value>(encoded).ok()?;
            interrupt_task_target(Some(&decoded))
        }
        _ => None,
    }
}

fn string_field<'a>(values: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| values.get(*key).and_then(Value::as_str))
}

fn consistent_string_field<'a>(
    values: &'a Map<String, Value>,
    keys: &[&str],
) -> std::result::Result<Option<&'a str>, ()> {
    let mut resolved = None;
    for key in keys {
        let Some(value) = values.get(*key) else {
            continue;
        };
        let Some(value) = value.as_str() else {
            return Err(());
        };
        if resolved.is_some_and(|existing| existing != value) {
            return Err(());
        }
        resolved = Some(value);
    }
    Ok(resolved)
}

fn reservation_has_pending_acceptance(reservation: &Reservation) -> bool {
    !reservation.spawn_failed && reservation.acceptance.iter().any(acceptance_blocks_turn)
}

fn acceptance_blocks_turn(check: &AcceptanceEntry) -> bool {
    match check.status {
        AcceptanceStatus::Passed => false,
        AcceptanceStatus::Pending | AcceptanceStatus::Failed => true,
        AcceptanceStatus::Unverifiable => check.release_notice_delivered_at_ms.is_none(),
    }
}

fn validate_unique_agent_bindings(ledger: &SessionLedger) -> Result<()> {
    let mut owners = BTreeMap::<&str, &str>::new();
    for (task_id, reservation) in &ledger.reservations {
        let Some(agent_hash) = reservation.agent_id_hash.as_deref() else {
            continue;
        };
        if let Some(existing) = owners.insert(agent_hash, task_id) {
            anyhow::bail!(
                "{AGENT_ID_COLLISION_ERROR_CODE}: Codey 子代理账本中的 agent_id 同时绑定任务 `{existing}` 与 `{task_id}`；已按 fail-closed 拒绝使用该账本"
            );
        }
    }
    Ok(())
}

fn identity_task_candidates(ledger: &SessionLedger, identifier: &str) -> BTreeSet<String> {
    let identifier_hash = hash_component(identifier);
    ledger
        .reservations
        .iter()
        .filter(|(task_id, reservation)| {
            reservation.agent_id_hash.as_deref() == Some(identifier_hash.as_str())
                || identifier_mentions_task(identifier, task_id)
        })
        .map(|(task_id, _)| task_id.clone())
        .collect()
}

/// Resolves the opaque Codex child thread id to the task path recorded in the
/// child's own rollout metadata. Codex exposes the thread id as `agent_id`, but
/// `agents.spawn_agent` returns only `/root/<task_name>`. The Hook's
/// `transcript_path` is therefore the only provider-owned object that contains
/// both values before the child's first tool call.
///
/// The transcript format is intentionally treated as a compatibility input:
/// every field and path relationship is checked, and any format drift simply
/// leaves the child unbound (fail-closed). No candidate-count or role-surface
/// heuristic is used.
fn task_id_from_subagent_transcript(
    state_root: &Path,
    session_id: &str,
    agent_id: &str,
    agent_type: Option<&str>,
    transcript_path: Option<&str>,
    ledger: &SessionLedger,
) -> Option<String> {
    let transcript_path = Path::new(transcript_path?);
    if !transcript_path.is_absolute()
        || transcript_path.extension().and_then(|value| value.to_str()) != Some("jsonl")
    {
        return None;
    }
    let metadata = fs::symlink_metadata(transcript_path).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let codex_home = state_root.parent()?;
    let sessions_root = fs::canonicalize(codex_home.join("sessions")).ok()?;
    let canonical_transcript = fs::canonicalize(transcript_path).ok()?;
    if !canonical_transcript.starts_with(&sessions_root)
        || !canonical_transcript
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.ends_with(&format!("-{agent_id}.jsonl")))
    {
        return None;
    }

    let reader = BufReader::new(File::open(&canonical_transcript).ok()?);
    let mut limited = reader.take((MAX_TRANSCRIPT_METADATA_LINE_BYTES + 1) as u64);
    let mut first_line = Vec::new();
    let read = limited.read_until(b'\n', &mut first_line).ok()?;
    if read == 0 || read > MAX_TRANSCRIPT_METADATA_LINE_BYTES {
        return None;
    }
    let value: Value = serde_json::from_slice(&first_line).ok()?;
    if value.get("type").and_then(Value::as_str) != Some("session_meta") {
        return None;
    }
    let payload = value.get("payload")?.as_object()?;
    let nested = payload
        .get("source")
        .and_then(|value| value.get("subagent"))
        .and_then(|value| value.get("thread_spawn"))
        .and_then(Value::as_object);
    let direct_parent =
        consistent_string_field(payload, &["parent_thread_id", "parentThreadId"]).ok()?;
    let nested_parent = match nested {
        Some(values) => {
            consistent_string_field(values, &["parent_thread_id", "parentThreadId"]).ok()?
        }
        None => None,
    };
    if consistent_string_field(payload, &["id"]).ok()? != Some(agent_id)
        || direct_parent != Some(session_id)
        || nested_parent.is_some_and(|parent| parent != session_id)
    {
        return None;
    }

    let direct_path = consistent_string_field(payload, &["agent_path", "agentPath"]).ok()?;
    let nested_path = match nested {
        Some(values) => consistent_string_field(values, &["agent_path", "agentPath"]).ok()?,
        None => None,
    };
    if direct_path.is_some() && nested_path.is_some() && direct_path != nested_path {
        return None;
    }
    let agent_path = direct_path.or(nested_path)?;
    let task_id = agent_path
        .split('/')
        .rfind(|component| !component.is_empty())?;
    if agent_path != format!("/root/{task_id}") {
        return None;
    }

    let reservation = ledger.reservations.get(task_id)?;
    if !reservation.state.is_active() || reservation.fenced_at_ms.is_some() {
        return None;
    }
    let direct_role = consistent_string_field(payload, &["agent_role", "agentRole"]).ok()?;
    let nested_role = match nested {
        Some(values) => consistent_string_field(values, &["agent_role", "agentRole"]).ok()?,
        None => None,
    };
    if direct_role.is_some() && nested_role.is_some() && direct_role != nested_role {
        return None;
    }
    let metadata_role = direct_role.or(nested_role)?;
    if metadata_role != reservation.role || agent_type.is_some_and(|role| role != metadata_role) {
        return None;
    }
    Some(task_id.to_string())
}

fn is_provisional_task_binding(bound_hash: &str, task_id: &str) -> bool {
    bound_hash == hash_component(task_id)
        || bound_hash == hash_component(&format!("/root/{task_id}"))
}

fn unique_task_for_identifier(ledger: &SessionLedger, identifier: &str) -> Result<Option<String>> {
    let candidates = identity_task_candidates(ledger, identifier);
    let candidate_list = candidates.iter().cloned().collect::<Vec<_>>().join(", ");
    anyhow::ensure!(
        candidates.len() <= 1,
        "{AGENT_ID_COLLISION_ERROR_CODE}: 标识 `{identifier}` 同时指向多个 attempt（{}），拒绝猜测主体归属",
        candidate_list
    );
    Ok(candidates.into_iter().next())
}

fn fence_identity_conflict(
    ledger: &mut SessionLedger,
    task_ids: &BTreeSet<String>,
    now_ms: u64,
    reason: &str,
) {
    for task_id in task_ids {
        let Some(reservation) = ledger.reservations.get_mut(task_id) else {
            continue;
        };
        reservation.agent_id_hash = None;
        if reservation.state.is_active() {
            reservation.state = ReservationState::Recovered;
            reservation.outcome = ExecutionOutcome::Lost;
            reservation.updated_at_ms = now_ms;
            reservation.completed_at_ms = Some(now_ms);
            reservation.fenced_at_ms = Some(now_ms);
            reservation.error_message = Some(reason.to_string());
        }
    }
}

fn identifier_mentions_task(identifier: &str, task_id: &str) -> bool {
    identifier == task_id || identifier == format!("/root/{task_id}")
}

fn normalized_identifier(value: &str) -> String {
    protocol::normalize_identifier(value)
}

fn canonical_value_hash(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).expect("serde_json::Value must always be serializable");
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn hash_component(value: &str) -> String {
    crate::fs_util::sha256_hex(value.as_bytes())
}

fn hash_component_bytes(value: &[u8]) -> String {
    crate::fs_util::sha256_hex(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn contract_input(task: &str, role: &str, mut contract: Value) -> Value {
        if let Some(values) = contract.as_object_mut()
            && !values.contains_key("capabilities")
        {
            values.insert(
                "capabilities".to_string(),
                if matches!(role, "codey_worker" | "codey_visual_worker") {
                    json!(["files.read", "workspace.write"])
                } else {
                    json!(["files.read"])
                },
            );
        }
        json!({
            "task_name": task,
            "agent_type": role,
            "fork_turns": "none",
            "message": format!("Do the task.\n{CONTRACT_PREFIX}{}", serde_json::to_string(&contract).unwrap())
        })
    }

    fn research_contract(task: &str) -> Value {
        json!({
            "id": task,
            "why": "breadth",
            "visual": false,
            "read": [],
            "write": [],
            "capabilities": ["files.read"],
            "checks": []
        })
    }

    fn worker_contract(task: &str, write: &str) -> Value {
        json!({
            "id": task,
            "why": "independent_work",
            "visual": false,
            "root": "/repo",
            "read": [],
            "write": [write],
            "capabilities": ["command.execute", "files.read", "workspace.write"],
            "checks": [{ "id": "tests", "cmd": "cargo test -p codey --lib" }]
        })
    }

    fn batch_decision_input(
        batch_number: u16,
        decision: RootBatchDecision,
        decision_id: &str,
    ) -> Value {
        json!({
            "decision": decision,
            "batch_number": batch_number,
            "decision_id": decision_id,
            "reason": "test decision"
        })
    }

    fn commit_batch_decision_for_test(
        state_root: &Path,
        session_id: &str,
        batch_number: u16,
        decision: RootBatchDecision,
        now_ms: u64,
    ) {
        let decision_id = format!("batch-{batch_number}-{now_ms}");
        let input = batch_decision_input(batch_number, decision, &decision_id);
        assert_eq!(
            prepare_batch_decision(state_root, "runtime-a", session_id, Some(&input), 0, now_ms,)
                .unwrap(),
            None
        );
        let response = json!({
            "structuredContent": {
                "accepted": true,
                "decision": decision,
                "batch_number": batch_number,
                "decision_id": decision_id,
                "reason": "test decision"
            }
        });
        assert_eq!(
            post_batch_decision(
                state_root,
                "runtime-a",
                session_id,
                Some(&input),
                Some(&response),
                now_ms + 1,
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn adaptive_contract_keeps_role_compatibility_without_dead_size_fields() {
        let input = contract_input("tiny", "codey_deep_research", research_contract("tiny"));
        assert!(prepare_contract(Some(&input)).is_ok());

        let quick = contract_input("quick", "codey_quick_scan", research_contract("quick"));
        assert!(prepare_contract(Some(&quick)).is_ok());

        let unknown = contract_input(
            "unknown",
            "codey_quick_scan",
            json!({
                "id": "unknown",
                "why": "guess",
                "visual": false
            }),
        );
        assert!(prepare_contract(Some(&unknown)).is_ok());

        let invalid_reason = contract_input(
            "invalid_reason",
            "codey_quick_scan",
            json!({
                "id": "invalid_reason",
                "why": "bad\nreason",
                "visual": false
            }),
        );
        assert!(
            prepare_contract(Some(&invalid_reason))
                .unwrap_err()
                .contains("审计说明")
        );

        let parallel = contract_input(
            "parallel",
            "codey_quick_scan",
            json!({
                "id": "parallel",
                "why": "parallel",
                "visual": false,
                "read": [],
                "write": [],
                "checks": []
            }),
        );
        assert!(prepare_contract(Some(&parallel)).is_ok());

        let retired_budget_fields = contract_input(
            "retired_budget_fields",
            "codey_quick_scan",
            json!({
                "id": "retired_budget_fields",
                "why": "scan",
                "budget_class": "parallel",
                "branch_calls": [3, 3],
                "visual": false
            }),
        );
        assert!(
            prepare_contract(Some(&retired_budget_fields))
                .unwrap_err()
                .contains("unknown field")
        );

        let user_requested = contract_input(
            "explicit",
            "codey_deep_research",
            json!({
                "id": "explicit",
                "why": "user_requested",
                "visual": false,
                "read": [],
                "write": [],
                "checks": []
            }),
        );
        assert!(prepare_contract(Some(&user_requested)).is_ok());

        let stale_size_fields = contract_input(
            "stale",
            "codey_deep_research",
            json!({
                "id": "stale",
                "why": "breadth",
                "calls": 4,
                "files": 5,
                "visual": false,
                "read": [],
                "write": [],
                "checks": []
            }),
        );
        assert!(
            prepare_contract(Some(&stale_size_fields))
                .unwrap_err()
                .contains("unknown field")
        );

        let legacy_contract = json!({
            "id": "legacy",
            "why": "breadth",
            "calls": 4,
            "files": 5,
            "dirs": 2,
            "large": false,
            "risk": false,
            "visual": false,
            "read": [],
            "write": [],
            "checks": []
        });
        let legacy_input = json!({
            "task_name": "legacy",
            "agent_type": "codey_deep_research",
            "fork_turns": "none",
            "message": format!(
                "Do the task.\n{LEGACY_CONTRACT_PREFIX_V1}{}",
                serde_json::to_string(&legacy_contract).unwrap()
            )
        });
        assert!(prepare_contract(Some(&legacy_input)).is_ok());
    }

    #[test]
    fn v2_capabilities_are_explicit_and_legacy_read_only_is_safely_upgraded() {
        let contract = json!({
            "id": "reader",
            "why": "breadth",
            "visual": false,
            "read": [],
            "write": [],
            "checks": []
        });
        let v2 = json!({
            "task_name": "reader",
            "agent_type": "codey_deep_research",
            "fork_turns": "none",
            "message": format!(
                "read\n{CONTRACT_PREFIX}{}",
                serde_json::to_string(&contract).unwrap()
            )
        });
        let v2_error = prepare_contract(Some(&v2)).unwrap_err();
        assert!(v2_error.contains("files.read"));
        assert!(v2_error.contains("\"capabilities\":[\"files.read\"]"));

        let v1 = json!({
            "task_name": "reader",
            "agent_type": "codey_deep_research",
            "fork_turns": "none",
            "message": format!(
                "read\n{LEGACY_CONTRACT_PREFIX_V1}{}",
                serde_json::to_string(&contract).unwrap()
            )
        });
        let prepared = prepare_contract(Some(&v1)).unwrap();
        assert_eq!(prepared.capabilities, ["files.read"]);

        let mut network_contract = contract;
        network_contract["capabilities"] = json!(["files.read", "network.access"]);
        let network = json!({
            "task_name": "reader",
            "agent_type": "codey_deep_research",
            "fork_turns": "none",
            "message": format!(
                "read\n{CONTRACT_PREFIX}{}",
                serde_json::to_string(&network_contract).unwrap()
            )
        });
        assert!(
            prepare_contract(Some(&network))
                .unwrap_err()
                .contains("未知 capability `network.access`")
        );

        let mut command_contract = research_contract("reader_command");
        command_contract["capabilities"] = json!(["files.read", "command.execute"]);
        let command_reader =
            contract_input("reader_command", "codey_deep_research", command_contract);
        assert!(
            prepare_contract(Some(&command_reader))
                .unwrap_err()
                .contains("只读角色不能声明 workspace.write 或 command.execute")
        );
    }

    #[test]
    fn contract_schemas_and_acceptance_limits_are_validated_and_persisted() {
        let temp = tempfile::tempdir().unwrap();
        let input_schema = json!({
            "type": "object",
            "required": ["query"],
            "properties": { "query": { "type": "string" } },
            "additionalProperties": false
        });
        let output_schema = json!({
            "type": "array",
            "items": { "type": "object", "required": ["path"] }
        });
        let mut contract = research_contract("schema_task");
        contract["input_schema"] = input_schema.clone();
        contract["output_schema"] = output_schema.clone();
        let input = contract_input("schema_task", "codey_deep_research", contract);
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 20).unwrap().unwrap();
        assert_eq!(
            ledger.reservations["schema_task"].input_schema.as_ref(),
            Some(&input_schema)
        );
        assert_eq!(
            ledger.reservations["schema_task"].output_schema.as_ref(),
            Some(&output_schema)
        );
        drop(ledger);
        drop(store);

        let invalid_schema = contract_input(
            "invalid_schema",
            "codey_deep_research",
            json!({
                "id": "invalid_schema",
                "why": "schema audit",
                "visual": false,
                "input_schema": { "type": "object", "required": "query" }
            }),
        );
        assert!(
            prepare_contract(Some(&invalid_schema))
                .unwrap_err()
                .contains("required")
        );

        let oversized_schema = contract_input(
            "oversized_schema",
            "codey_deep_research",
            json!({
                "id": "oversized_schema",
                "why": "schema audit",
                "visual": false,
                "input_schema": {
                    "type": "object",
                    "description": "x".repeat(MAX_SCHEMA_BYTES)
                }
            }),
        );
        assert!(
            prepare_contract(Some(&oversized_schema))
                .unwrap_err()
                .contains("4096 字节")
        );

        let checks = (0..MAX_ACCEPTANCE_CHECKS)
            .map(|index| json!({ "id": format!("check_{index}"), "cmd": "cargo check" }))
            .collect::<Vec<_>>();
        let mut eight_checks = worker_contract("eight_checks", "backend/src");
        eight_checks["checks"] = Value::Array(checks);
        assert!(
            prepare_contract_with_workspace(
                Some(&contract_input(
                    "eight_checks",
                    "codey_worker",
                    eight_checks
                )),
                Some("/repo")
            )
            .is_ok()
        );

        let nine_checks = (0..=MAX_ACCEPTANCE_CHECKS)
            .map(|index| json!({ "id": format!("check_{index}"), "cmd": "cargo check" }))
            .collect::<Vec<_>>();
        let mut too_many = worker_contract("too_many", "backend/src");
        too_many["checks"] = Value::Array(nine_checks);
        assert!(
            prepare_contract_with_workspace(
                Some(&contract_input("too_many", "codey_worker", too_many)),
                Some("/repo")
            )
            .unwrap_err()
            .contains("最多 8 项")
        );

        let long_checks = (0..5)
            .map(|index| json!({ "id": format!("long_{index}"), "cmd": "x".repeat(900) }))
            .collect::<Vec<_>>();
        let mut too_long = worker_contract("too_long", "backend/src");
        too_long["checks"] = Value::Array(long_checks);
        assert!(
            prepare_contract_with_workspace(
                Some(&contract_input("too_long", "codey_worker", too_long)),
                Some("/repo")
            )
            .unwrap_err()
            .contains("总长度不能超过 4096")
        );
    }

    #[test]
    fn encrypted_message_is_read_only_and_rejects_write_roles() {
        let encrypted_worker = json!({
            "task_name": "encrypted_worker",
            "agent_type": "codey_worker",
            "fork_turns": "none",
            "message": format!("gAAAAA{}", "A".repeat(160))
        });
        let error =
            prepare_contract_with_workspace(Some(&encrypted_worker), Some("/repo")).unwrap_err();
        assert!(error.contains("无法验证 write ownership"));

        let encrypted_read = json!({
            "task_name": "encrypted_read",
            "agent_type": "codey_quick_scan",
            "fork_turns": "none",
            "message": format!("gAAAAA{}", "A".repeat(160))
        });
        let prepared =
            prepare_contract_with_workspace(Some(&encrypted_read), Some("/repo")).unwrap();
        assert_eq!(prepared.contract.id, "encrypted_read");
        assert_eq!(prepared.contract.reason, "encrypted_message");
        assert_eq!(prepared.workspace_root.as_deref(), Some("/repo"));
        assert!(prepared.write_paths.is_empty());
        assert_eq!(prepared.read_paths, ["/repo"]);
        assert!(prepared.native_read_scope);
        assert!(prepared.contract.acceptance.is_empty());

        assert!(prepare_contract_with_workspace(Some(&encrypted_read), None).is_ok());

        let missing_contract = json!({
            "task_name": "plain_worker",
            "agent_type": "codey_worker",
            "fork_turns": "none",
            "message": "Plain text without a delegation contract"
        });
        assert!(
            prepare_contract_with_workspace(Some(&missing_contract), Some("/repo"))
                .unwrap_err()
                .contains("最后一行缺少 CODEY_DELEGATION_V2")
        );
    }

    #[test]
    fn opaque_native_read_scope_is_serialized_against_all_writers() {
        let temp = tempfile::tempdir().unwrap();
        let encrypted_read = json!({
            "task_name": "encrypted_reader",
            "agent_type": "codey_quick_scan",
            "fork_turns": "none",
            "message": format!("gAAAAA{}", "A".repeat(160))
        });
        assert_eq!(
            pre_spawn_with_workspace(
                temp.path(),
                "runtime-a",
                "reader-first-session",
                Some(&encrypted_read),
                Some("/repo"),
                0,
                10,
            )
            .unwrap(),
            None
        );

        let mut external_worker = worker_contract("external_worker", ".");
        external_worker["root"] = json!("/external-repo");
        let external_worker = contract_input("external_worker", "codey_worker", external_worker);
        let denial = pre_spawn_with_workspace(
            temp.path(),
            "runtime-a",
            "reader-first-session",
            Some(&external_worker),
            Some("/repo"),
            1,
            20,
        )
        .unwrap()
        .unwrap();
        assert!(denial.contains("具体 read scope 对 Hook 不可见"));

        assert_eq!(
            pre_spawn_with_workspace(
                temp.path(),
                "runtime-a",
                "writer-first-session",
                Some(&external_worker),
                Some("/repo"),
                0,
                30,
            )
            .unwrap(),
            None
        );
        let denial = pre_spawn_with_workspace(
            temp.path(),
            "runtime-a",
            "writer-first-session",
            Some(&encrypted_read),
            Some("/repo"),
            1,
            40,
        )
        .unwrap()
        .unwrap();
        assert!(denial.contains("具体 read scope 对 Hook 不可见"));
    }

    #[test]
    fn plaintext_contract_claims_may_span_roots_without_hook_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let write = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );

        assert_eq!(
            pre_spawn_with_workspace(
                temp.path(),
                "runtime-a",
                "session-a",
                Some(&write),
                None,
                0,
                10,
            )
            .unwrap(),
            None
        );

        let mut absolute_without_root = worker_contract("worker_absolute", "/external/src");
        absolute_without_root
            .as_object_mut()
            .unwrap()
            .remove("root");
        let absolute_without_root =
            contract_input("worker_absolute", "codey_worker", absolute_without_root);
        assert_eq!(
            pre_spawn_with_workspace(
                temp.path(),
                "runtime-a",
                "session-absolute-without-root",
                Some(&absolute_without_root),
                Some("relative-hook-cwd"),
                0,
                10,
            )
            .unwrap(),
            None
        );

        assert_eq!(
            pre_spawn_with_workspace(
                temp.path(),
                "runtime-a",
                "session-external-root",
                Some(&write),
                Some("/other"),
                0,
                10,
            )
            .unwrap(),
            None
        );

        let nested_root = contract_input(
            "worker_b",
            "codey_worker",
            json!({
                "id": "worker_b",
                "why": "independent_work",
                "visual": false,
                "root": "/repo/sub",
                "read": [],
                "write": ["backend/src"],
                "checks": [{ "id": "tests", "cmd": "cargo test -p codey --lib" }]
            }),
        );
        assert_eq!(
            pre_spawn_with_workspace(
                temp.path(),
                "runtime-a",
                "session-nested-root",
                Some(&nested_root),
                Some("/repo"),
                0,
                10,
            )
            .unwrap(),
            None
        );

        let outside_root = contract_input(
            "worker_c",
            "codey_worker",
            worker_contract("worker_c", "/etc/passwd"),
        );
        assert_eq!(
            pre_spawn_with_workspace(
                temp.path(),
                "runtime-a",
                "session-outside-claim",
                Some(&outside_root),
                Some("/repo"),
                0,
                10,
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn explicit_external_root_is_not_confused_with_hook_cwd() {
        let mut read_contract = research_contract("external_reader");
        read_contract["root"] = json!("/external-repo");
        read_contract["read"] = json!(["docs"]);
        let read_input = contract_input("external_reader", "codey_deep_research", read_contract);
        let prepared = prepare_contract_with_workspace(Some(&read_input), Some("/repo")).unwrap();
        assert_eq!(prepared.workspace_root.as_deref(), Some("/external-repo"));
        assert_eq!(prepared.read_paths, vec!["/external-repo/docs"]);

        let mut write_contract = worker_contract("external_worker", ".");
        write_contract["root"] = json!("/external-repo");
        let write_input = contract_input("external_worker", "codey_worker", write_contract);
        let prepared = prepare_contract_with_workspace(Some(&write_input), Some("/repo")).unwrap();
        assert_eq!(prepared.workspace_root.as_deref(), Some("/external-repo"));
        assert_eq!(prepared.write_paths, vec!["/external-repo"]);
    }

    #[test]
    fn read_only_contract_root_is_independent_from_the_hook_workspace() {
        let read_only = |root: &str, read: &[&str]| {
            contract_input(
                "research_a",
                "codey_deep_research",
                json!({
                    "id": "research_a",
                    "why": "breadth",
                    "visual": false,
                    "root": root,
                    "read": read,
                    "write": [],
                    "checks": []
                }),
            )
        };

        assert!(prepare_contract_with_workspace(Some(&read_only("/repo", &[])), None).is_ok());
        assert!(
            prepare_contract_with_workspace(
                Some(&read_only("/repo", &["/repo/docs"])),
                Some("/repo")
            )
            .is_ok()
        );
        assert!(
            prepare_contract_with_workspace(Some(&read_only("/other", &[])), Some("/repo")).is_ok()
        );
        assert!(
            prepare_contract_with_workspace(
                Some(&read_only("/repo", &["/outside"])),
                Some("/repo")
            )
            .is_ok()
        );
    }

    #[test]
    fn concurrency_allows_three_verified_read_only_agents() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..READ_ONLY_CONCURRENCY_LIMIT {
            let input = json!({
                "task_name": format!("opaque_{index}"),
                "agent_type": "codey_quick_scan",
                "fork_turns": "none",
                "message": format!("gAAAAA{}", "A".repeat(160))
            });
            assert_eq!(
                pre_spawn(
                    temp.path(),
                    "runtime-a",
                    "session-a",
                    Some(&input),
                    index,
                    u64::try_from(index).unwrap(),
                )
                .unwrap(),
                None
            );
        }

        let fourth = json!({
            "task_name": "opaque_fourth",
            "agent_type": "codey_quick_scan",
            "fork_turns": "none",
            "message": format!("gAAAAA{}", "A".repeat(160))
        });
        let denial = pre_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&fourth),
            READ_ONLY_CONCURRENCY_LIMIT,
            100,
        )
        .unwrap()
        .unwrap();
        assert!(denial.contains(CONCURRENCY_LIMIT_ERROR_CODE));
        assert!(denial.contains("纯只读批次"));
        assert!(denial.contains("并发上限 3"));
    }

    #[test]
    fn write_or_mixed_concurrency_remains_bounded_at_two() {
        let temp = tempfile::tempdir().unwrap();
        for (index, path) in ["backend/a.rs", "frontend/b.ts"].into_iter().enumerate() {
            let task = format!("worker_{index}");
            let input = contract_input(&task, "codey_worker", worker_contract(&task, path));
            assert_eq!(
                pre_spawn(
                    temp.path(),
                    "runtime-a",
                    "session-a",
                    Some(&input),
                    index,
                    u64::try_from(index).unwrap(),
                )
                .unwrap(),
                None
            );
        }

        let third = contract_input(
            "worker_third",
            "codey_worker",
            worker_contract("worker_third", "docs/c.md"),
        );
        let denial = pre_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&third),
            WRITE_OR_MIXED_CONCURRENCY_LIMIT,
            10,
        )
        .unwrap()
        .unwrap();
        assert!(denial.contains(CONCURRENCY_LIMIT_ERROR_CODE));
        assert!(denial.contains("包含写入型或身份未确认代理"));
        assert!(denial.contains("并发上限 2"));
    }

    #[test]
    fn settled_batches_can_continue_beyond_the_legacy_root_turn_budget() {
        let temp = tempfile::tempdir().unwrap();
        let mut now_ms = 0_u64;
        const BATCHES: u16 = 4;

        for batch_number in 1..=BATCHES {
            now_ms += 1;
            let input = json!({
                "task_name": format!("batch_{batch_number}_task"),
                "agent_type": "codey_quick_scan",
                "fork_turns": "none",
                "message": format!("gAAAAA{}", "A".repeat(160))
            });
            assert_eq!(
                pre_spawn(
                    temp.path(),
                    "runtime-a",
                    "session-a",
                    Some(&input),
                    0,
                    now_ms,
                )
                .unwrap(),
                None
            );

            let store = LedgerStore::open(temp.path(), "session-a").unwrap();
            let ledger = store
                .load("runtime-a", "session-a", now_ms + 1)
                .unwrap()
                .unwrap();
            assert_eq!(ledger.batch_number, batch_number);
            assert_eq!(
                ledger
                    .reservations
                    .values()
                    .filter(|reservation| reservation.batch_number == batch_number)
                    .count(),
                1
            );
            drop(ledger);
            drop(store);

            now_ms += 2;
            observe_status_response(temp.path(), "runtime-a", "session-a", None, true, now_ms)
                .unwrap();
            if batch_number < BATCHES {
                now_ms += 1;
                commit_batch_decision_for_test(
                    temp.path(),
                    "session-a",
                    batch_number,
                    RootBatchDecision::SpawnNextBatch,
                    now_ms,
                );
                now_ms += 1;
            }
        }
    }

    #[test]
    fn settled_batch_requires_a_committed_structured_decision() {
        let temp = tempfile::tempdir().unwrap();
        let spawn = contract_input(
            "research_a",
            "codey_deep_research",
            research_contract("research_a"),
        );
        assert_eq!(
            pre_spawn(temp.path(), "runtime-a", "session-a", Some(&spawn), 0, 10,).unwrap(),
            None
        );
        observe_status_response(temp.path(), "runtime-a", "session-a", None, true, 20).unwrap();

        let prompt = open_batch_decision_if_settled(temp.path(), "runtime-a", "session-a", 0, 21)
            .unwrap()
            .unwrap();
        assert!(prompt.contains("第 1 批"));
        assert!(prompt.contains(crate::subagent_control_mcp::QUALIFIED_TOOL_NAME));
        let root_denial = pre_root_tool(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&json!({ "command": "cargo check" })),
            22,
        )
        .unwrap()
        .unwrap();
        assert!(root_denial.contains("批次决策"));

        let decision = batch_decision_input(1, RootBatchDecision::ContinueRoot, "batch-1-continue");
        assert_eq!(
            prepare_batch_decision(
                temp.path(),
                "runtime-a",
                "session-a",
                Some(&decision),
                0,
                30,
            )
            .unwrap(),
            None
        );
        let failed_receipt = post_batch_decision(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&decision),
            Some(&json!({
                "isError": true,
                "structuredContent": {
                    "accepted": true,
                    "decision": "continue_root",
                    "batch_number": 1,
                    "decision_id": "batch-1-continue",
                    "reason": "test decision"
                },
                "content": [{
                    "type": "text",
                    "text": "{\"accepted\":true,\"decision\":\"continue_root\",\"batch_number\":1,\"decision_id\":\"batch-1-continue\",\"reason\":\"test decision\"}"
                }]
            })),
            31,
        )
        .unwrap()
        .unwrap();
        assert!(failed_receipt.contains("决策未提交"));

        assert_eq!(
            prepare_batch_decision(
                temp.path(),
                "runtime-a",
                "session-a",
                Some(&decision),
                0,
                32,
            )
            .unwrap(),
            None
        );
        let receipt = json!({
            "structuredContent": {
                "accepted": true,
                "decision": "continue_root",
                "batch_number": 1,
                "decision_id": "batch-1-continue",
                "reason": "test decision"
            }
        });
        assert_eq!(
            post_batch_decision(
                temp.path(),
                "runtime-a",
                "session-a",
                Some(&decision),
                Some(&receipt),
                33,
            )
            .unwrap(),
            None
        );
        assert_eq!(
            pre_root_tool(
                temp.path(),
                "runtime-a",
                "session-a",
                Some(&json!({ "command": "cargo check" })),
                34,
            )
            .unwrap(),
            None
        );
        assert!(
            batch_decision_stop_reason(temp.path(), "runtime-a", "session-a", 35)
                .unwrap()
                .unwrap()
                .contains("continue_root")
        );

        commit_batch_decision_for_test(
            temp.path(),
            "session-a",
            1,
            RootBatchDecision::Complete,
            40,
        );
        assert_eq!(
            batch_decision_stop_reason(temp.path(), "runtime-a", "session-a", 42).unwrap(),
            None
        );
    }

    #[test]
    fn settled_batch_without_a_control_tool_has_a_bounded_fail_closed_exit() {
        let temp = tempfile::tempdir().unwrap();
        let spawn = contract_input(
            "research_a",
            "codey_deep_research",
            research_contract("research_a"),
        );
        assert_eq!(
            pre_spawn(temp.path(), "runtime-a", "session-a", Some(&spawn), 0, 10).unwrap(),
            None
        );
        observe_status_response(temp.path(), "runtime-a", "session-a", None, true, 20).unwrap();

        for now_ms in [21, 22] {
            let reason = batch_decision_stop_reason(temp.path(), "runtime-a", "session-a", now_ms)
                .unwrap()
                .unwrap();
            assert!(reason.contains(crate::subagent_control_mcp::QUALIFIED_TOOL_NAME));
        }
        assert_eq!(
            batch_decision_stop_reason(temp.path(), "runtime-a", "session-a", 23).unwrap(),
            None
        );

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 24).unwrap().unwrap();
        assert!(matches!(
            ledger.batch_decision,
            BatchDecisionState::ControlPlaneFailed {
                batch_number: 1,
                failure_kind: BatchDecisionControlFailureKind::NoProgress,
                failure_count: MAX_BATCH_DECISION_CONTROL_FAILURES,
                ..
            }
        ));
        drop(ledger);
        drop(store);

        let root_denial = pre_root_tool(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&json!({ "command": "cargo check" })),
            25,
        )
        .unwrap()
        .unwrap();
        assert!(root_denial.contains(BATCH_DECISION_CONTROL_FAILURE_ERROR_CODE));

        let retry_decision = batch_decision_input(1, RootBatchDecision::Complete, "late-complete");
        let decision_denial = prepare_batch_decision(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&retry_decision),
            0,
            26,
        )
        .unwrap()
        .unwrap();
        assert!(decision_denial.contains(BATCH_DECISION_CONTROL_FAILURE_ERROR_CODE));

        let next_spawn = contract_input(
            "research_b",
            "codey_deep_research",
            research_contract("research_b"),
        );
        let spawn_denial = pre_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&next_spawn),
            0,
            27,
        )
        .unwrap()
        .unwrap();
        assert!(spawn_denial.contains(BATCH_DECISION_CONTROL_FAILURE_ERROR_CODE));

        settle_turn(temp.path(), "runtime-a", "session-a", 28).unwrap();
        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        assert!(store.load("runtime-a", "session-a", 29).unwrap().is_none());
    }

    #[test]
    fn invalid_batch_decision_receipts_fail_closed_after_a_bounded_retry_count() {
        let temp = tempfile::tempdir().unwrap();
        let spawn = contract_input(
            "research_a",
            "codey_deep_research",
            research_contract("research_a"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&spawn), 0, 10).unwrap();
        observe_status_response(temp.path(), "runtime-a", "session-a", None, true, 20).unwrap();
        let decision = batch_decision_input(1, RootBatchDecision::Complete, "batch-1-complete");

        for failure_index in 0..MAX_BATCH_DECISION_CONTROL_FAILURES {
            let now_ms = 30 + u64::from(failure_index) * 2;
            assert_eq!(
                prepare_batch_decision(
                    temp.path(),
                    "runtime-a",
                    "session-a",
                    Some(&decision),
                    0,
                    now_ms,
                )
                .unwrap(),
                None
            );
            let reason = post_batch_decision(
                temp.path(),
                "runtime-a",
                "session-a",
                Some(&decision),
                Some(&json!({ "isError": true })),
                now_ms + 1,
            )
            .unwrap()
            .unwrap();
            if failure_index + 1 == MAX_BATCH_DECISION_CONTROL_FAILURES {
                assert!(reason.contains(BATCH_DECISION_CONTROL_FAILURE_ERROR_CODE));
            } else {
                assert!(reason.contains("决策未提交"));
            }
        }

        assert_eq!(
            batch_decision_stop_reason(temp.path(), "runtime-a", "session-a", 40).unwrap(),
            None
        );
        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 41).unwrap().unwrap();
        assert!(matches!(
            ledger.batch_decision,
            BatchDecisionState::ControlPlaneFailed {
                failure_kind: BatchDecisionControlFailureKind::InvalidReceipt,
                failure_count: MAX_BATCH_DECISION_CONTROL_FAILURES,
                ..
            }
        ));
    }

    #[test]
    fn failed_encrypted_spawns_remain_recorded_without_a_budget_cap() {
        let temp = tempfile::tempdir().unwrap();
        let attempts = 8_u16;
        for index in 0..attempts {
            let input = json!({
                "task_name": format!("failed_opaque_{index}"),
                "agent_type": "codey_quick_scan",
                "fork_turns": "none",
                "message": format!("gAAAAA{}", "A".repeat(160))
            });
            assert_eq!(
                pre_spawn(
                    temp.path(),
                    "runtime-a",
                    "session-a",
                    Some(&input),
                    0,
                    u64::from(index) * 10,
                )
                .unwrap(),
                None
            );
            post_spawn(
                temp.path(),
                "runtime-a",
                "session-a",
                Some(&input),
                Some(&Value::String(
                    "collab spawn failed: agent thread limit reached".to_string(),
                )),
                u64::from(index) * 10 + 1,
            )
            .unwrap();
        }

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 110).unwrap().unwrap();
        assert_eq!(ledger.reservations.len(), usize::from(attempts));
        assert_eq!(
            ledger
                .reservations
                .values()
                .filter(|reservation| reservation.spawn_failed)
                .count(),
            usize::from(attempts)
        );
    }

    #[test]
    fn ledger_denies_overlapping_write_claims_and_survives_reload() {
        let temp = tempfile::tempdir().unwrap();
        let first = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );
        assert_eq!(
            pre_spawn(temp.path(), "runtime-a", "session-a", Some(&first), 0, 10).unwrap(),
            None
        );
        let second = contract_input(
            "worker_b",
            "codey_worker",
            worker_contract("worker_b", "backend/src/lib.rs"),
        );
        let denial = pre_spawn(temp.path(), "runtime-a", "session-a", Some(&second), 0, 20)
            .unwrap()
            .unwrap();
        assert!(denial.contains("资源冲突"));

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 30).unwrap().unwrap();
        assert_eq!(ledger.reservations.len(), 1);
        assert!(ledger.reservations.contains_key("worker_a"));
    }

    #[test]
    fn terminal_write_ownership_stays_reserved_until_acceptance_passes() {
        let temp = tempfile::tempdir().unwrap();
        let first = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&first), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&first),
            Some(&json!({ "agent_id": "agent-a" })),
            20,
        )
        .unwrap();
        subagent_stopped(temp.path(), "runtime-a", "session-a", "agent-a", 30).unwrap();

        let second = contract_input(
            "worker_b",
            "codey_worker",
            worker_contract("worker_b", "backend/src/lib.rs"),
        );
        let denial = pre_spawn(temp.path(), "runtime-a", "session-a", Some(&second), 1, 40)
            .unwrap()
            .unwrap();
        assert!(denial.contains("资源冲突"));
        assert!(denial.contains("worker_a"));
    }

    #[test]
    fn concurrent_reservations_are_serialized_without_lost_updates() {
        use std::sync::{Arc, Barrier};

        let temp = tempfile::tempdir().unwrap();
        let state_root = temp.path().to_path_buf();
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for (task, path, now_ms) in [
            ("worker_a", "backend/a.rs", 10),
            ("worker_b", "backend/b.rs", 20),
        ] {
            let state_root = state_root.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let input = contract_input(task, "codey_worker", worker_contract(task, path));
                barrier.wait();
                pre_spawn(
                    &state_root,
                    "runtime-a",
                    "session-a",
                    Some(&input),
                    1,
                    now_ms,
                )
            }));
        }
        barrier.wait();
        for handle in handles {
            assert_eq!(handle.join().unwrap().unwrap(), None);
        }

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 30).unwrap().unwrap();
        assert_eq!(ledger.reservations.len(), 2);
    }

    #[test]
    fn unrelated_sessions_do_not_share_the_same_ledger_lock() {
        use std::sync::mpsc;
        use std::time::Duration;

        let temp = tempfile::tempdir().unwrap();
        let held_lock_path = temp.path().join(format!(
            "{LEDGER_LOCK_FILE}.{}",
            hash_component("session-a")
        ));
        let held_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(held_lock_path)
            .unwrap();
        held_lock.lock_exclusive().unwrap();

        let root = temp.path().to_path_buf();
        let (sender, receiver) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            sender.send(LedgerStore::open(&root, "session-b")).unwrap();
        });
        let other_store = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("an unrelated session must not wait for session-a's lock")
            .unwrap();
        drop(other_store);
        FileExt::unlock(&held_lock).unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn failed_spawn_keeps_terminal_reservation_and_task_id() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/a.rs"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&json!({ "isError": true, "error": "capacity" })),
            20,
        )
        .unwrap();
        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 30).unwrap().unwrap();
        assert_eq!(
            ledger.reservations["worker_a"].state,
            ReservationState::Terminal
        );
        assert_eq!(
            ledger.reservations["worker_a"].outcome,
            ExecutionOutcome::Failed
        );
        assert!(ledger.reservations["worker_a"].spawn_failed);
        assert!(ledger.issued_task_ids.contains("worker_a"));

        drop(ledger);
        drop(store);
        let denial = pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 40)
            .unwrap()
            .unwrap();
        assert!(denial.contains(DUPLICATE_TASK_ID_ERROR_CODE));
        assert!(denial.contains("任务 ID `worker_a` 已在本轮编排账本中"));
        assert!(denial.contains("账本状态为 `failed`"));
        assert!(denial.contains("默认由主代理接管"));
        assert!(denial.contains("`CODEY_DELEGATION_V2.id`"));
        assert!(denial.contains("不要把本次拒绝当作完成后立即 Stop"));
    }

    #[test]
    fn duplicate_denial_requires_reconciliation_for_pending_and_running_spawns() {
        for (index, response, expected_state, state_label) in [
            (0, None, ReservationState::Pending, "`pending`"),
            (
                1,
                Some(json!({ "agent_id": "agent-running" })),
                ReservationState::Running,
                "`running`",
            ),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let task = format!("research_{index}");
            let input = contract_input(&task, "codey_deep_research", research_contract(&task));
            pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
            post_spawn(
                temp.path(),
                "runtime-a",
                "session-a",
                Some(&input),
                response.as_ref(),
                20,
            )
            .unwrap();

            let store = LedgerStore::open(temp.path(), "session-a").unwrap();
            let ledger = store.load("runtime-a", "session-a", 30).unwrap().unwrap();
            assert_eq!(ledger.reservations[&task].state, expected_state);
            drop(ledger);
            drop(store);

            let denial = pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 1, 40)
                .unwrap()
                .unwrap();
            assert!(denial.contains(DUPLICATE_TASK_ID_ERROR_CODE));
            assert!(denial.contains(state_label));
            assert!(denial.contains("不带筛选的 `agents.list_agents` 对账"));
            assert!(denial.contains("不要重发旧 ID"));
            assert!(denial.contains("Stop"));
        }
    }

    #[test]
    fn duplicate_terminal_task_consumes_the_existing_result_instead_of_respawning() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "research_a",
            "codey_deep_research",
            research_contract("research_a"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&json!({ "agent_id": "/root/research_a" })),
            20,
        )
        .unwrap();
        subagent_stopped(
            temp.path(),
            "runtime-a",
            "session-a",
            "/root/research_a",
            30,
        )
        .unwrap();

        let denial = pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 40)
            .unwrap()
            .unwrap();
        assert!(denial.contains(DUPLICATE_TASK_ID_ERROR_CODE));
        assert!(denial.contains("进入终态或恢复态"));
        assert!(denial.contains("消费已有结果"));
        assert!(denial.contains("不得重新派生"));
    }

    #[test]
    fn followup_requires_a_bound_running_attempt_and_fails_before_reactivation() {
        let temp = tempfile::tempdir().unwrap();
        let target = json!({
            "target": "/root/worker_followup",
            "message": "continue"
        });

        let missing = pre_followup_task(
            temp.path(),
            "runtime-a",
            "missing-session",
            Some(&target),
            10,
        )
        .unwrap()
        .unwrap();
        assert!(missing.contains(FOLLOWUP_REQUIRES_ACTIVE_ATTEMPT_ERROR_CODE));
        assert!(missing.contains("唤醒子代理前拒绝"));
        assert!(missing.contains("全新的 task_name"));
        assert!(missing.contains("spawn_next_batch"));

        let spawn = contract_input(
            "worker_followup",
            "codey_worker",
            worker_contract("worker_followup", "backend/src/lib.rs"),
        );
        pre_spawn(
            temp.path(),
            "runtime-a",
            "active-session",
            Some(&spawn),
            0,
            20,
        )
        .unwrap();
        let agent_id = "/root/worker_followup";
        post_spawn(
            temp.path(),
            "runtime-a",
            "active-session",
            Some(&spawn),
            Some(&json!({ "task_name": agent_id })),
            30,
        )
        .unwrap();
        subagent_started(temp.path(), "runtime-a", "active-session", agent_id, 35).unwrap();
        assert_eq!(
            pre_followup_task(
                temp.path(),
                "runtime-a",
                "active-session",
                Some(&target),
                40,
            )
            .unwrap(),
            None
        );

        subagent_stopped(temp.path(), "runtime-a", "active-session", agent_id, 50).unwrap();
        let settled = pre_followup_task(
            temp.path(),
            "runtime-a",
            "active-session",
            Some(&target),
            60,
        )
        .unwrap()
        .unwrap();
        assert!(settled.contains(FOLLOWUP_REQUIRES_ACTIVE_ATTEMPT_ERROR_CODE));
        assert!(settled.contains("state=Terminal"));
        assert!(settled.contains("不要等待旧 canonical task 自行恢复"));

        let pending_spawn = contract_input(
            "pending_followup",
            "codey_deep_research",
            research_contract("pending_followup"),
        );
        pre_spawn(
            temp.path(),
            "runtime-a",
            "pending-session",
            Some(&pending_spawn),
            0,
            70,
        )
        .unwrap();
        let pending_target = json!({
            "target": "/root/pending_followup",
            "message": "continue"
        });
        let pending = pre_followup_task(
            temp.path(),
            "runtime-a",
            "pending-session",
            Some(&pending_target),
            80,
        )
        .unwrap()
        .unwrap();
        assert!(pending.contains("仍为 pending"));
        assert!(pending.contains("`agents.list_agents`"));
    }

    #[test]
    fn root_interrupt_abandons_only_the_target_and_cannot_be_reactivated() {
        let temp = tempfile::tempdir().unwrap();
        let session_id = "interrupt-session";
        for (index, task_id) in ["reader_a", "reader_b"].into_iter().enumerate() {
            let spawn = contract_input(task_id, "codey_deep_research", research_contract(task_id));
            pre_spawn(
                temp.path(),
                "runtime-a",
                session_id,
                Some(&spawn),
                index,
                10 + index as u64,
            )
            .unwrap();
            post_spawn(
                temp.path(),
                "runtime-a",
                session_id,
                Some(&spawn),
                Some(&json!({ "agent_id": format!("agent-{task_id}") })),
                20 + index as u64,
            )
            .unwrap();
        }

        let interrupt = json!({ "target": "/root/reader_a" });
        let abandoned = abandon_interrupted_reservation(
            temp.path(),
            "runtime-a",
            session_id,
            Some(&interrupt),
            30,
        )
        .unwrap()
        .unwrap();
        assert!(abandoned.changed);
        assert_eq!(
            abandoned.agent_id_hash.as_deref(),
            Some(hash_component("agent-reader_a").as_str())
        );

        let followup_a = json!({ "target": "/root/reader_a" });
        let followup_b = json!({ "target": "agent-reader_b" });
        assert!(
            pre_followup_task(temp.path(), "runtime-a", session_id, Some(&followup_a), 31,)
                .unwrap()
                .unwrap()
                .contains(FOLLOWUP_REQUIRES_ACTIVE_ATTEMPT_ERROR_CODE)
        );
        assert_eq!(
            pre_followup_task(temp.path(), "runtime-a", session_id, Some(&followup_b), 32,)
                .unwrap(),
            None
        );

        let duplicate = Value::String(serde_json::to_string(&interrupt).unwrap());
        assert!(
            !abandon_interrupted_reservation(
                temp.path(),
                "runtime-a",
                session_id,
                Some(&duplicate),
                33,
            )
            .unwrap()
            .unwrap()
            .changed
        );
        subagent_stopped(temp.path(), "runtime-a", session_id, "agent-reader_a", 34).unwrap();

        let store = LedgerStore::open(temp.path(), session_id).unwrap();
        let ledger = store.load("runtime-a", session_id, 35).unwrap().unwrap();
        let abandoned = &ledger.reservations["reader_a"];
        assert_eq!(abandoned.state, ReservationState::Recovered);
        assert_eq!(abandoned.outcome, ExecutionOutcome::Lost);
        assert!(
            abandoned
                .error_message
                .as_deref()
                .is_some_and(|reason| reason.contains("permanently abandoned"))
        );
        assert_eq!(
            ledger.reservations["reader_b"].state,
            ReservationState::Running
        );
    }

    #[test]
    fn duplicate_or_late_post_spawn_events_do_not_regress_reservation_state() {
        let temp = tempfile::tempdir().unwrap();
        let failed = contract_input(
            "worker_failed",
            "codey_worker",
            worker_contract("worker_failed", "backend/a.rs"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&failed), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&failed),
            Some(&json!({ "isError": true, "error": "capacity" })),
            20,
        )
        .unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&failed),
            Some(&json!({ "agent_id": "late-agent" })),
            30,
        )
        .unwrap();

        let completed = contract_input(
            "worker_completed",
            "codey_worker",
            worker_contract("worker_completed", "backend/b.rs"),
        );
        pre_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&completed),
            0,
            40,
        )
        .unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&completed),
            Some(&json!({ "agent_id": "completed-agent" })),
            50,
        )
        .unwrap();
        subagent_stopped(temp.path(), "runtime-a", "session-a", "completed-agent", 60).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&completed),
            Some(&json!({ "isError": true, "error": "late failure" })),
            70,
        )
        .unwrap();

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 80).unwrap().unwrap();
        assert_eq!(
            ledger.reservations["worker_failed"].state,
            ReservationState::Terminal
        );
        assert_eq!(
            ledger.reservations["worker_failed"].outcome,
            ExecutionOutcome::Failed
        );
        assert_eq!(
            ledger.reservations["worker_completed"].state,
            ReservationState::Terminal
        );
        assert_eq!(ledger.reservations.len(), 2);
    }

    #[test]
    fn authoritative_status_refines_an_unknown_lifecycle_stop_outcome() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "research_done",
            "codey_deep_research",
            research_contract("research_done"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&json!({ "agent_id": "agent-done" })),
            20,
        )
        .unwrap();
        subagent_stopped(temp.path(), "runtime-a", "session-a", "agent-done", 30).unwrap();

        let response = Value::String(
            serde_json::to_string(&json!({
                "updates": [{
                    "agent_id": "agent-done",
                    "status": "completed"
                }]
            }))
            .unwrap(),
        );
        observe_status_response(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&response),
            false,
            40,
        )
        .unwrap();

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 50).unwrap().unwrap();
        let reservation = &ledger.reservations["research_done"];
        assert_eq!(reservation.state, ReservationState::Terminal);
        assert_eq!(reservation.outcome, ExecutionOutcome::Succeeded);
        assert_eq!(reservation.completed_at_ms, Some(30));
        assert!(reservation.error_message.is_none());
        assert!(reservation.agent_id_hash.is_none());
    }

    #[test]
    fn textual_failed_spawn_marks_the_reservation_failed() {
        for (index, response) in [
            Value::String("collab spawn failed: agent thread limit reached".to_string()),
            json!({
                "content": [{
                    "type": "text",
                    "text": "collab spawn failed: agent thread limit reached"
                }]
            }),
            json!({ "isError": true, "error": "capacity" }),
            Value::String(
                serde_json::to_string(&json!({ "isError": true, "error": "capacity" })).unwrap(),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let temp = tempfile::tempdir().unwrap();
            let task = format!("worker_{index}");
            let input = contract_input(
                &task,
                "codey_worker",
                worker_contract(&task, "backend/a.rs"),
            );
            pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
            post_spawn(
                temp.path(),
                "runtime-a",
                "session-a",
                Some(&input),
                Some(&response),
                20,
            )
            .unwrap();

            let store = LedgerStore::open(temp.path(), "session-a").unwrap();
            let ledger = store.load("runtime-a", "session-a", 30).unwrap().unwrap();
            assert_eq!(ledger.reservations[&task].state, ReservationState::Terminal);
            assert_eq!(ledger.reservations[&task].outcome, ExecutionOutcome::Failed);
            assert!(ledger.reservations[&task].spawn_failed);
        }
    }

    #[test]
    fn task_ids_remain_unique_after_advancing_to_a_new_batch() {
        let temp = tempfile::tempdir().unwrap();
        let first = contract_input(
            "research_a",
            "codey_deep_research",
            research_contract("research_a"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&first), 0, 10).unwrap();
        observe_status_response(temp.path(), "runtime-a", "session-a", None, true, 20).unwrap();
        commit_batch_decision_for_test(
            temp.path(),
            "session-a",
            1,
            RootBatchDecision::SpawnNextBatch,
            21,
        );

        let second = contract_input(
            "research_b",
            "codey_deep_research",
            research_contract("research_b"),
        );
        assert_eq!(
            pre_spawn(temp.path(), "runtime-a", "session-a", Some(&second), 0, 30,).unwrap(),
            None
        );
        let denial = pre_spawn(temp.path(), "runtime-a", "session-a", Some(&first), 1, 40)
            .unwrap()
            .unwrap();
        assert!(denial.contains("任务 ID `research_a` 已在本轮编排账本中"));

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 50).unwrap().unwrap();
        assert_eq!(ledger.batch_number, 2);
        assert_eq!(ledger.reservations.len(), 2);
    }

    #[test]
    fn spawn_response_with_agent_id_ignores_error_fields() {
        for (index, response) in [
            json!({
                "agent_id": "agent-a",
                "isError": true,
                "error": "capacity"
            }),
            json!({
                "result": {
                    "agent_id": "agent-a",
                    "error": "subagent payload reports a handled failure"
                }
            }),
        ]
        .into_iter()
        .enumerate()
        {
            let temp = tempfile::tempdir().unwrap();
            let task = format!("worker_{index}");
            let input = contract_input(
                &task,
                "codey_worker",
                worker_contract(&task, "backend/a.rs"),
            );
            pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
            post_spawn(
                temp.path(),
                "runtime-a",
                "session-a",
                Some(&input),
                Some(&response),
                20,
            )
            .unwrap();

            let store = LedgerStore::open(temp.path(), "session-a").unwrap();
            let ledger = store.load("runtime-a", "session-a", 30).unwrap().unwrap();
            assert_eq!(ledger.reservations[&task].state, ReservationState::Running);
        }
    }

    #[test]
    fn spawn_response_without_agent_id_remains_pending_until_lifecycle_confirmation() {
        for (index, response) in [
            None,
            Some(json!({
                "result": { "error": "deeply nested diagnostics" }
            })),
        ]
        .into_iter()
        .enumerate()
        {
            let temp = tempfile::tempdir().unwrap();
            let task = format!("worker_{index}");
            let input = contract_input(
                &task,
                "codey_worker",
                worker_contract(&task, "backend/a.rs"),
            );
            pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
            post_spawn(
                temp.path(),
                "runtime-a",
                "session-a",
                Some(&input),
                response.as_ref(),
                20,
            )
            .unwrap();

            let store = LedgerStore::open(temp.path(), "session-a").unwrap();
            let ledger = store.load("runtime-a", "session-a", 30).unwrap().unwrap();
            assert_eq!(ledger.reservations[&task].state, ReservationState::Pending);
            drop(ledger);
            drop(store);

            let agent_id = format!("/root/{task}");
            subagent_started(temp.path(), "runtime-a", "session-a", &agent_id, 40).unwrap();
            let store = LedgerStore::open(temp.path(), "session-a").unwrap();
            let ledger = store.load("runtime-a", "session-a", 50).unwrap().unwrap();
            assert_eq!(ledger.reservations[&task].state, ReservationState::Running);
            assert_eq!(
                ledger.reservations[&task].agent_id_hash.as_deref(),
                Some(hash_component(&agent_id).as_str())
            );
        }
    }

    #[test]
    fn successful_spawn_identifier_wins_over_failure_like_text() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/a.rs"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&json!({
                "agent_id": "agent-a",
                "message": "failed to spawn agent in a previous attempt"
            })),
            20,
        )
        .unwrap();

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 30).unwrap().unwrap();
        assert_eq!(
            ledger.reservations["worker_a"].state,
            ReservationState::Running
        );
    }

    #[test]
    fn matching_spawn_task_name_wins_over_failure_like_text() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "worker_task_receipt",
            "codey_worker",
            worker_contract("worker_task_receipt", "backend/a.rs"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        let response = Value::String(
            serde_json::to_string(&json!({
                "task_name": "/root/worker_task_receipt",
                "message": "failed to spawn agent in a previous attempt"
            }))
            .unwrap(),
        );
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&response),
            20,
        )
        .unwrap();

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 30).unwrap().unwrap();
        let reservation = &ledger.reservations["worker_task_receipt"];
        assert_eq!(reservation.state, ReservationState::Running);
        assert_eq!(
            reservation.agent_id_hash.as_deref(),
            Some(hash_component("/root/worker_task_receipt").as_str())
        );
    }

    #[test]
    fn task_name_response_requires_exact_lifecycle_identity() {
        let temp = tempfile::tempdir().unwrap();
        let worker = contract_input(
            "worker_taskname",
            "codey_worker",
            worker_contract("worker_taskname", "backend/src"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&worker), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&worker),
            Some(&json!({ "task_name": "/root/worker_taskname" })),
            20,
        )
        .unwrap();

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 25).unwrap().unwrap();
        assert_eq!(
            ledger.reservations["worker_taskname"].state,
            ReservationState::Running
        );
        assert_eq!(
            ledger.reservations["worker_taskname"]
                .agent_id_hash
                .as_deref(),
            Some(hash_component("/root/worker_taskname").as_str())
        );
        drop(ledger);
        drop(store);

        assert_eq!(
            authorize_child_tool(
                temp.path(),
                "runtime-a",
                "session-a",
                "/root/worker_taskname",
                "mcp__codey_fastctx__grep",
                Some(&json!({ "path": "backend/src", "pattern": "worker" })),
                30,
            )
            .unwrap(),
            None
        );

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 35).unwrap().unwrap();
        assert_eq!(
            ledger.reservations["worker_taskname"]
                .agent_id_hash
                .as_deref(),
            Some(hash_component("/root/worker_taskname").as_str())
        );
        drop(ledger);
        drop(store);

        let research = contract_input(
            "research_pending",
            "codey_deep_research",
            research_contract("research_pending"),
        );
        pre_spawn(
            temp.path(),
            "runtime-a",
            "session-b",
            Some(&research),
            0,
            40,
        )
        .unwrap();
        subagent_started(
            temp.path(),
            "runtime-a",
            "session-b",
            "/root/unrelated_task",
            50,
        )
        .unwrap();

        let store = LedgerStore::open(temp.path(), "session-b").unwrap();
        let ledger = store.load("runtime-a", "session-b", 60).unwrap().unwrap();
        assert_eq!(
            ledger.reservations["research_pending"].state,
            ReservationState::Pending
        );
        assert!(
            ledger.reservations["research_pending"]
                .agent_id_hash
                .is_none()
        );

        drop(ledger);
        drop(store);

        let nested = contract_input(
            "nested_pending",
            "codey_deep_research",
            research_contract("nested_pending"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-c", Some(&nested), 0, 70).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-c",
            Some(&nested),
            Some(&json!({
                "output": {
                    "task_name": "/root/nested_pending"
                }
            })),
            80,
        )
        .unwrap();
        let store = LedgerStore::open(temp.path(), "session-c").unwrap();
        let ledger = store.load("runtime-a", "session-c", 90).unwrap().unwrap();
        assert_eq!(
            ledger.reservations["nested_pending"].state,
            ReservationState::Pending
        );
        assert!(
            ledger.reservations["nested_pending"]
                .agent_id_hash
                .is_none()
        );
    }

    #[test]
    fn canonical_task_match_requires_the_exact_root_task_path() {
        let temp = tempfile::tempdir().unwrap();
        let root_named = contract_input("root", "codey_deep_research", research_contract("root"));
        let worker = contract_input(
            "worker_final",
            "codey_deep_research",
            research_contract("worker_final"),
        );
        pre_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&root_named),
            0,
            10,
        )
        .unwrap();
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&worker), 1, 11).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&worker),
            Some(&json!({ "task_name": "/root/worker_final" })),
            20,
        )
        .unwrap();

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 30).unwrap().unwrap();
        assert_eq!(ledger.reservations["root"].state, ReservationState::Pending);
        assert!(ledger.reservations["root"].agent_id_hash.is_none());
        assert_eq!(
            ledger.reservations["worker_final"].state,
            ReservationState::Running
        );
        assert!(identifier_mentions_task("worker_final", "worker_final"));
        assert!(identifier_mentions_task(
            "/root/worker_final",
            "worker_final"
        ));
        assert!(!identifier_mentions_task(
            "/root/root/worker_final",
            "worker_final"
        ));
        assert!(!identifier_mentions_task(
            "foreign:worker_final",
            "worker_final"
        ));
    }

    #[test]
    fn transcript_binding_requires_consistent_parent_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let state_root = temp.path().join("codey-subagent-gate-v3");
        std::fs::create_dir_all(&state_root).unwrap();
        let task_id = "metadata_reader";
        let session_id = "parent-session";
        let input = contract_input(task_id, "codey_deep_research", research_contract(task_id));
        pre_spawn(&state_root, "runtime-a", session_id, Some(&input), 0, 10).unwrap();
        let store = LedgerStore::open(&state_root, session_id).unwrap();
        let ledger = store.load("runtime-a", session_id, 20).unwrap().unwrap();

        let write_transcript = |agent_id: &str, payload: Value| {
            let path = temp
                .path()
                .join("sessions/2026/08/20")
                .join(format!("rollout-probe-{agent_id}.jsonl"));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                format!(
                    "{}\n",
                    serde_json::to_string(&json!({
                        "type": "session_meta",
                        "payload": payload
                    }))
                    .unwrap()
                ),
            )
            .unwrap();
            path
        };

        let conflicting_alias_id = "01a01d5b-dead-beef-baad-000000000010";
        let conflicting_alias = write_transcript(
            conflicting_alias_id,
            json!({
                "id": conflicting_alias_id,
                "parent_thread_id": session_id,
                "parentThreadId": "different-parent",
                "agent_path": format!("/root/{task_id}"),
                "agent_role": "codey_deep_research"
            }),
        );
        assert_eq!(
            task_id_from_subagent_transcript(
                &state_root,
                session_id,
                conflicting_alias_id,
                Some("codey_deep_research"),
                conflicting_alias.to_str(),
                &ledger,
            ),
            None
        );

        let conflicting_nested_id = "01a01d5b-dead-beef-baad-000000000011";
        let conflicting_nested = write_transcript(
            conflicting_nested_id,
            json!({
                "id": conflicting_nested_id,
                "parent_thread_id": session_id,
                "agent_path": format!("/root/{task_id}"),
                "agent_role": "codey_deep_research",
                "source": { "subagent": { "thread_spawn": {
                    "parent_thread_id": "different-parent",
                    "agent_path": format!("/root/{task_id}"),
                    "agent_role": "codey_deep_research"
                }}}
            }),
        );
        assert_eq!(
            task_id_from_subagent_transcript(
                &state_root,
                session_id,
                conflicting_nested_id,
                Some("codey_deep_research"),
                conflicting_nested.to_str(),
                &ledger,
            ),
            None
        );

        let valid_id = "01a01d5b-dead-beef-baad-000000000012";
        let valid = write_transcript(
            valid_id,
            json!({
                "id": valid_id,
                "parent_thread_id": session_id,
                "agent_path": format!("/root/{task_id}"),
                "agent_role": "codey_deep_research",
                "source": { "subagent": { "thread_spawn": {
                    "parent_thread_id": session_id,
                    "agent_path": format!("/root/{task_id}"),
                    "agent_role": "codey_deep_research"
                }}}
            }),
        );
        assert_eq!(
            task_id_from_subagent_transcript(
                &state_root,
                session_id,
                valid_id,
                Some("codey_deep_research"),
                valid.to_str(),
                &ledger,
            ),
            Some(task_id.to_string())
        );
    }

    #[test]
    fn duplicate_agent_identity_fences_every_conflicting_attempt() {
        let temp = tempfile::tempdir().unwrap();
        let first = contract_input(
            "worker_first",
            "codey_worker",
            worker_contract("worker_first", "backend/first"),
        );
        let second = contract_input(
            "worker_second",
            "codey_worker",
            worker_contract("worker_second", "backend/second"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&first), 0, 10).unwrap();
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&second), 1, 11).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&first),
            Some(&json!({ "agent_id": "shared-agent" })),
            20,
        )
        .unwrap();
        let error = post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&second),
            Some(&json!({ "agent_id": "shared-agent" })),
            30,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains(AGENT_ID_COLLISION_ERROR_CODE));

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 40).unwrap().unwrap();
        for reservation in ledger.reservations.values() {
            assert_eq!(reservation.state, ReservationState::Recovered);
            assert_eq!(reservation.outcome, ExecutionOutcome::Lost);
            assert!(reservation.agent_id_hash.is_none());
            assert_eq!(reservation.fenced_at_ms, Some(30));
        }
    }

    #[test]
    fn ambiguous_terminal_identity_fences_every_candidate() {
        let temp = tempfile::tempdir().unwrap();
        for (index, task) in ["shared_agent", "research_b"].into_iter().enumerate() {
            let input = contract_input(task, "codey_deep_research", research_contract(task));
            pre_spawn(
                temp.path(),
                "runtime-a",
                "session-a",
                Some(&input),
                index,
                10 + index as u64,
            )
            .unwrap();
        }
        // Model a corrupted/cross-protocol ledger where one provider identity is
        // also another reservation's canonical task id. Status reconciliation
        // must fence both candidates instead of picking either interpretation.
        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let mut ledger = store.load("runtime-a", "session-a", 20).unwrap().unwrap();
        let reservation = ledger.reservations.get_mut("research_b").unwrap();
        reservation.state = ReservationState::Running;
        reservation.agent_id_hash = Some(hash_component("shared_agent"));
        reservation.started_at_ms = Some(20);
        store.save(&mut ledger, 20).unwrap();
        drop(ledger);
        drop(store);

        let response = json!({
            "updates": [{
                "agent_id": "shared_agent",
                "status": "completed"
            }]
        });
        let error = observe_status_response(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&response),
            false,
            30,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains(AGENT_ID_COLLISION_ERROR_CODE));

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 40).unwrap().unwrap();
        assert!(ledger.reservations.values().all(|reservation| {
            reservation.state == ReservationState::Recovered
                && reservation.outcome == ExecutionOutcome::Lost
                && reservation.fenced_at_ms == Some(30)
        }));
    }

    #[test]
    fn expired_attempt_is_fenced_and_cannot_be_revived_by_late_events() {
        let temp = tempfile::tempdir().unwrap();
        let mut contract = worker_contract("worker_deadline", "backend/src");
        contract["deadline_ms"] = json!(100);
        let input = contract_input("worker_deadline", "codey_worker", contract);
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&json!({ "task_name": "/root/worker_deadline" })),
            20,
        )
        .unwrap();

        assert_eq!(
            active_reservation_count(temp.path(), "runtime-a", "session-a", 110).unwrap(),
            Some(0)
        );
        subagent_started(
            temp.path(),
            "runtime-a",
            "session-a",
            "/root/worker_deadline",
            120,
        )
        .unwrap();
        let denial = authorize_child_tool(
            temp.path(),
            "runtime-a",
            "session-a",
            "/root/worker_deadline",
            "Bash",
            Some(&json!({ "command": "cargo test" })),
            130,
        )
        .unwrap()
        .unwrap();
        assert!(denial.contains("已终态、过期或被 fence"));

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 140).unwrap().unwrap();
        let reservation = &ledger.reservations["worker_deadline"];
        assert_eq!(reservation.state, ReservationState::Terminal);
        assert_eq!(reservation.outcome, ExecutionOutcome::TimedOut);
        assert_eq!(reservation.completed_at_ms, Some(110));
        assert_eq!(reservation.fenced_at_ms, Some(110));
        assert!(reservation.agent_id_hash.is_none());
    }

    #[test]
    fn ledger_lock_contention_fails_within_the_bounded_deadline() {
        let temp = tempfile::tempdir().unwrap();
        let held = LedgerStore::open(temp.path(), "session-a").unwrap();
        let started = Instant::now();
        let error = LedgerStore::open(temp.path(), "session-a")
            .err()
            .expect("second open should time out while the session lock is held");
        assert!(format!("{error:#}").contains("账本锁超时"));
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(held);
        assert!(LedgerStore::open(temp.path(), "session-a").is_ok());
    }

    #[test]
    fn windows_lock_violation_is_only_treated_as_contention_on_windows() {
        assert_eq!(
            ledger_lock_is_contended(&std::io::Error::from_raw_os_error(33)),
            cfg!(windows)
        );
    }

    #[test]
    fn child_write_paths_and_commands_defer_to_codex_after_capability_check() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&json!({ "agent_id": "agent-a" })),
            15,
        )
        .unwrap();
        let allowed_patch = json!({
            "patch": "*** Begin Patch\n*** Update File: backend/src/lib.rs\n*** End Patch"
        });
        assert_eq!(
            authorize_child_tool(
                temp.path(),
                "runtime-a",
                "session-a",
                "agent-a",
                "apply_patch",
                Some(&allowed_patch),
                20,
            )
            .unwrap(),
            None
        );
        let outside_declared_claim = json!({
            "patch": "*** Begin Patch\n*** Update File: README.md\n*** End Patch"
        });
        assert_eq!(
            authorize_child_tool(
                temp.path(),
                "runtime-a",
                "session-a",
                "agent-a",
                "apply_patch",
                Some(&outside_declared_claim),
                30,
            )
            .unwrap(),
            None
        );
        assert_eq!(
            authorize_child_tool(
                temp.path(),
                "runtime-a",
                "session-a",
                "agent-a",
                "mcp__codey_fastctx__replace",
                Some(&json!({ "opaque_target": "provider-specific" })),
                35,
            )
            .unwrap(),
            None
        );

        assert_eq!(
            authorize_child_tool(
                temp.path(),
                "runtime-a",
                "session-a",
                "agent-a",
                "Bash",
                Some(&json!({ "command": "cargo test --lib" })),
                40,
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn runtime_authorization_rechecks_capability_and_tool_provenance() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&json!({ "agent_id": "agent-a" })),
            20,
        )
        .unwrap();

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let mut ledger = store.load("runtime-a", "session-a", 25).unwrap().unwrap();
        ledger
            .reservations
            .get_mut("worker_a")
            .unwrap()
            .capabilities
            .retain(|capability| capability != "workspace.write");
        store.save(&mut ledger, 26).unwrap();
        drop(ledger);
        drop(store);

        let patch = json!({
            "patch": "*** Begin Patch\n*** Update File: backend/src/lib.rs\n*** End Patch"
        });
        let capability_denial = authorize_child_tool(
            temp.path(),
            "runtime-a",
            "session-a",
            "agent-a",
            "apply_patch",
            Some(&patch),
            30,
        )
        .unwrap()
        .unwrap();
        assert!(capability_denial.contains("workspace.write"));

        let provenance_denial = authorize_child_tool(
            temp.path(),
            "runtime-a",
            "session-a",
            "agent-a",
            "mcp__evil__replace",
            Some(&json!({ "path": "backend/src/lib.rs" })),
            40,
        )
        .unwrap()
        .unwrap();
        assert!(provenance_denial.contains("规则"));
    }

    #[test]
    fn child_read_paths_defer_to_codex_after_capability_check() {
        let temp = tempfile::tempdir().unwrap();
        let mut contract = research_contract("research_scoped");
        contract["root"] = json!("/repo");
        contract["read"] = json!(["backend/src"]);
        let input = contract_input("research_scoped", "codey_deep_research", contract);
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&json!({ "agent_id": "agent-reader" })),
            20,
        )
        .unwrap();

        assert_eq!(
            authorize_child_tool(
                temp.path(),
                "runtime-a",
                "session-a",
                "agent-reader",
                "mcp__codey_fastctx__grep",
                Some(&json!({ "path": "backend/src", "pattern": "needle" })),
                30,
            )
            .unwrap(),
            None
        );
        assert_eq!(
            authorize_child_tool(
                temp.path(),
                "runtime-a",
                "session-a",
                "agent-reader",
                "mcp__codey_fastctx__inspect_local_file",
                Some(&json!({ "file_path": "/another-repo/README.md" })),
                40,
            )
            .unwrap(),
            None
        );

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let mut ledger = store.load("runtime-a", "session-a", 45).unwrap().unwrap();
        ledger
            .reservations
            .get_mut("research_scoped")
            .unwrap()
            .capabilities
            .push("workspace.write".to_string());
        store.save(&mut ledger, 46).unwrap();
        drop(ledger);
        drop(store);

        let denial = authorize_child_tool(
            temp.path(),
            "runtime-a",
            "session-a",
            "agent-reader",
            "apply_patch",
            Some(&json!({
                "patch": "*** Begin Patch\n*** Update File: README.md\n*** End Patch"
            })),
            50,
        )
        .unwrap()
        .unwrap();
        assert!(denial.contains("不是可写角色"));
    }

    #[test]
    fn explicit_external_claims_are_coordination_metadata_not_runtime_acls() {
        let temp = tempfile::tempdir().unwrap();
        let mut contract = research_contract("external_reader");
        contract["root"] = json!("/external-repo");
        contract["read"] = json!(["docs"]);
        let input = contract_input("external_reader", "codey_deep_research", contract);
        assert_eq!(
            pre_spawn_with_workspace(
                temp.path(),
                "runtime-a",
                "external-read-session",
                Some(&input),
                Some("/repo"),
                0,
                10,
            )
            .unwrap(),
            None
        );
        post_spawn(
            temp.path(),
            "runtime-a",
            "external-read-session",
            Some(&input),
            Some(&json!({ "agent_id": "agent-external-reader" })),
            20,
        )
        .unwrap();

        let read = json!({ "file_path": "/external-repo/docs/guide.md" });
        assert_eq!(
            authorize_child_tool_with_context(
                temp.path(),
                "runtime-a",
                "external-read-session",
                ChildToolContext {
                    agent_id: "agent-external-reader",
                    agent_type: None,
                    transcript_path: None,
                    tool_name: "mcp__codey_fastctx__inspect_local_file",
                    tool_input: Some(&read),
                },
                30,
            )
            .unwrap(),
            None
        );
        let undeclared_read = json!({ "file_path": "/another-repo/secret.txt" });
        assert_eq!(
            authorize_child_tool_with_context(
                temp.path(),
                "runtime-a",
                "external-read-session",
                ChildToolContext {
                    agent_id: "agent-external-reader",
                    agent_type: None,
                    transcript_path: None,
                    tool_name: "mcp__codey_fastctx__inspect_local_file",
                    tool_input: Some(&undeclared_read),
                },
                35,
            )
            .unwrap(),
            None
        );

        let mut write_contract = worker_contract("external_worker", ".");
        write_contract["root"] = json!("/external-repo");
        let write_input = contract_input("external_worker", "codey_worker", write_contract);
        assert_eq!(
            pre_spawn_with_workspace(
                temp.path(),
                "runtime-a",
                "external-write-session",
                Some(&write_input),
                Some("/repo"),
                0,
                40,
            )
            .unwrap(),
            None
        );
        post_spawn(
            temp.path(),
            "runtime-a",
            "external-write-session",
            Some(&write_input),
            Some(&json!({ "agent_id": "agent-external-worker" })),
            50,
        )
        .unwrap();
        let replace = json!({ "path": "/external-repo/src/lib.rs", "pattern": "old" });
        assert_eq!(
            authorize_child_tool_with_context(
                temp.path(),
                "runtime-a",
                "external-write-session",
                ChildToolContext {
                    agent_id: "agent-external-worker",
                    agent_type: None,
                    transcript_path: None,
                    tool_name: "mcp__codey_fastctx__replace",
                    tool_input: Some(&replace),
                },
                60,
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn child_read_paths_do_not_depend_on_contract_root() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let worktree_a = workspace.join(".worktrees/a");
        let worktree_b = workspace.join(".worktrees/b");
        fs::create_dir_all(worktree_a.join("backend/src")).unwrap();
        fs::create_dir_all(worktree_b.join("backend/src")).unwrap();
        let workspace = workspace.to_string_lossy().into_owned();
        let worktree_a = worktree_a.to_string_lossy().into_owned();
        let worktree_b = worktree_b.to_string_lossy().into_owned();

        let mut contract = research_contract("worktree_reader");
        contract["root"] = json!(worktree_a);
        contract["read"] = json!(["backend/src"]);
        let input = contract_input("worktree_reader", "codey_deep_research", contract);
        pre_spawn_with_workspace(
            temp.path(),
            "runtime-a",
            "worktree-session",
            Some(&input),
            Some(&workspace),
            0,
            10,
        )
        .unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "worktree-session",
            Some(&input),
            Some(&json!({ "agent_id": "agent-worktree" })),
            20,
        )
        .unwrap();

        let relative_read = json!({ "path": "backend/src", "pattern": "needle" });
        assert_eq!(
            authorize_child_tool_with_context(
                temp.path(),
                "runtime-a",
                "worktree-session",
                ChildToolContext {
                    agent_id: "agent-worktree",
                    agent_type: None,
                    transcript_path: None,
                    tool_name: "mcp__codey_fastctx__grep",
                    tool_input: Some(&relative_read),
                },
                30,
            )
            .unwrap(),
            None
        );

        let absolute_read = json!({
            "file_path": format!("{worktree_a}/backend/src/lib.rs")
        });
        assert_eq!(
            authorize_child_tool_with_context(
                temp.path(),
                "runtime-a",
                "worktree-session",
                ChildToolContext {
                    agent_id: "agent-worktree",
                    agent_type: None,
                    transcript_path: None,
                    tool_name: "mcp__codey_fastctx__inspect_local_file",
                    tool_input: Some(&absolute_read),
                },
                35,
            )
            .unwrap(),
            None
        );

        assert_eq!(
            authorize_child_tool_with_context(
                temp.path(),
                "runtime-a",
                "worktree-session",
                ChildToolContext {
                    agent_id: "agent-worktree",
                    agent_type: None,
                    transcript_path: None,
                    tool_name: "mcp__codey_fastctx__grep",
                    tool_input: Some(&relative_read),
                },
                40,
            )
            .unwrap(),
            None
        );

        let sibling_absolute_read = json!({
            "file_path": format!("{worktree_b}/backend/src/lib.rs")
        });
        assert_eq!(
            authorize_child_tool_with_context(
                temp.path(),
                "runtime-a",
                "worktree-session",
                ChildToolContext {
                    agent_id: "agent-worktree",
                    agent_type: None,
                    transcript_path: None,
                    tool_name: "mcp__codey_fastctx__inspect_local_file",
                    tool_input: Some(&sibling_absolute_read),
                },
                45,
            )
            .unwrap(),
            None
        );

        assert_eq!(
            authorize_child_tool_with_context(
                temp.path(),
                "runtime-a",
                "worktree-session",
                ChildToolContext {
                    agent_id: "agent-worktree",
                    agent_type: None,
                    transcript_path: None,
                    tool_name: "mcp__codey_fastctx__grep",
                    tool_input: Some(&relative_read),
                },
                50,
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn parent_checkout_claim_does_not_block_a_child_worktree_read() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let worktree = workspace.join(".worktrees/child");
        fs::create_dir_all(worktree.join("backend/src")).unwrap();
        fs::create_dir_all(workspace.join("backend/src")).unwrap();
        let workspace = workspace.to_string_lossy().into_owned();
        let worktree = worktree.to_string_lossy().into_owned();

        let mut contract = research_contract("misrooted_reader");
        contract["root"] = json!(workspace);
        contract["read"] = json!(["backend/src"]);
        let input = contract_input("misrooted_reader", "codey_deep_research", contract);
        pre_spawn_with_workspace(
            temp.path(),
            "runtime-a",
            "misrooted-session",
            Some(&input),
            Some(&workspace),
            0,
            10,
        )
        .unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "misrooted-session",
            Some(&input),
            Some(&json!({ "agent_id": "agent-misrooted" })),
            20,
        )
        .unwrap();

        let relative_read = json!({ "path": "backend/src", "pattern": "needle" });
        assert_eq!(
            authorize_child_tool_with_context(
                temp.path(),
                "runtime-a",
                "misrooted-session",
                ChildToolContext {
                    agent_id: "agent-misrooted",
                    agent_type: None,
                    transcript_path: None,
                    tool_name: "mcp__codey_fastctx__grep",
                    tool_input: Some(&relative_read),
                },
                30,
            )
            .unwrap(),
            None
        );

        let absolute_read = json!({
            "file_path": format!("{worktree}/backend/src/lib.rs")
        });
        assert_eq!(
            authorize_child_tool_with_context(
                temp.path(),
                "runtime-a",
                "misrooted-session",
                ChildToolContext {
                    agent_id: "agent-misrooted",
                    agent_type: None,
                    transcript_path: None,
                    tool_name: "mcp__codey_fastctx__inspect_local_file",
                    tool_input: Some(&absolute_read),
                },
                40,
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn explicit_contract_records_sibling_worktree_claims() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let checkout = workspace.join("checkout");
        let worktree = workspace.join(".worktrees/child");
        fs::create_dir_all(checkout.join("backend/src")).unwrap();
        fs::create_dir_all(worktree.join("backend/src")).unwrap();
        let checkout = checkout.to_string_lossy().into_owned();
        let worktree = worktree.to_string_lossy().into_owned();

        let mut contract = research_contract("sibling_worktree_reader");
        contract["root"] = json!(worktree);
        contract["read"] = json!(["backend/src"]);
        let input = contract_input("sibling_worktree_reader", "codey_deep_research", contract);
        assert_eq!(
            pre_spawn_with_workspace(
                temp.path(),
                "runtime-a",
                "sibling-worktree-session",
                Some(&input),
                Some(&checkout),
                0,
                10,
            )
            .unwrap(),
            None
        );
        post_spawn(
            temp.path(),
            "runtime-a",
            "sibling-worktree-session",
            Some(&input),
            Some(&json!({ "agent_id": "agent-sibling-worktree" })),
            20,
        )
        .unwrap();

        let read = json!({
            "file_path": format!("{worktree}/backend/src/lib.rs")
        });
        assert_eq!(
            authorize_child_tool_with_context(
                temp.path(),
                "runtime-a",
                "sibling-worktree-session",
                ChildToolContext {
                    agent_id: "agent-sibling-worktree",
                    agent_type: None,
                    transcript_path: None,
                    tool_name: "mcp__codey_fastctx__inspect_local_file",
                    tool_input: Some(&read),
                },
                30,
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn pending_binding_never_guesses_from_unique_or_equivalent_authorization() {
        let temp = tempfile::tempdir().unwrap();
        for (task, read) in [("research_a", "backend/a"), ("research_b", "backend/b")] {
            let mut contract = research_contract(task);
            contract["root"] = json!("/repo");
            contract["read"] = json!([read]);
            let input = contract_input(task, "codey_deep_research", contract);
            pre_spawn(
                temp.path(),
                "runtime-a",
                "ambiguous-session",
                Some(&input),
                usize::from(task == "research_b"),
                10,
            )
            .unwrap();
        }
        let denial = authorize_child_tool_with_context(
            temp.path(),
            "runtime-a",
            "ambiguous-session",
            ChildToolContext {
                agent_id: "opaque-agent",
                agent_type: Some("codey_deep_research"),
                transcript_path: None,
                tool_name: "mcp__codey_fastctx__grep",
                tool_input: Some(&json!({ "path": "backend/a", "pattern": "needle" })),
            },
            20,
        )
        .unwrap()
        .unwrap();
        assert!(denial.contains(UNBOUND_ATTEMPT_ERROR_CODE));

        for task in ["equivalent_a", "equivalent_b"] {
            let mut contract = research_contract(task);
            contract["root"] = json!("/repo");
            contract["read"] = json!(["backend/shared"]);
            let input = contract_input(task, "codey_deep_research", contract);
            pre_spawn(
                temp.path(),
                "runtime-a",
                "equivalent-session",
                Some(&input),
                usize::from(task == "equivalent_b"),
                30,
            )
            .unwrap();
        }
        for agent in ["opaque-agent-a", "opaque-agent-b"] {
            let denial = authorize_child_tool_with_context(
                temp.path(),
                "runtime-a",
                "equivalent-session",
                ChildToolContext {
                    agent_id: agent,
                    agent_type: Some("codey_deep_research"),
                    transcript_path: None,
                    tool_name: "mcp__codey_fastctx__grep",
                    tool_input: Some(&json!({ "path": "backend/shared", "pattern": "needle" })),
                },
                40,
            )
            .unwrap()
            .unwrap();
            assert!(denial.contains(UNBOUND_ATTEMPT_ERROR_CODE));
        }
        let store = LedgerStore::open(temp.path(), "equivalent-session").unwrap();
        let ledger = store
            .load("runtime-a", "equivalent-session", 50)
            .unwrap()
            .unwrap();
        assert!(ledger.reservations.values().all(|reservation| {
            reservation.state == ReservationState::Pending && reservation.agent_id_hash.is_none()
        }));
    }

    #[test]
    fn role_mismatch_is_fenced_before_pending_identity_can_bind() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "research_a",
            "codey_deep_research",
            research_contract("research_a"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();

        let denial = authorize_child_tool_with_context(
            temp.path(),
            "runtime-a",
            "session-a",
            ChildToolContext {
                agent_id: "/root/research_a",
                agent_type: Some("codey_worker"),
                transcript_path: None,
                tool_name: "mcp__codey_fastctx__grep",
                tool_input: Some(&json!({ "path": ".", "pattern": "needle" })),
            },
            20,
        )
        .unwrap()
        .unwrap();
        assert!(denial.contains(AGENT_ID_COLLISION_ERROR_CODE));

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 30).unwrap().unwrap();
        let reservation = &ledger.reservations["research_a"];
        assert_eq!(reservation.state, ReservationState::Recovered);
        assert_eq!(reservation.outcome, ExecutionOutcome::Lost);
        assert!(reservation.agent_id_hash.is_none());
        assert_eq!(reservation.fenced_at_ms, Some(20));
    }

    #[test]
    fn verbatim_windows_paths_are_normalized_without_allowing_globs() {
        let drive = normalize_absolute_path(r"\\?\C:\repo\src").unwrap();
        let unc = normalize_absolute_path(r"\\?\UNC\server\share\src").unwrap();
        if cfg!(windows) {
            assert_eq!(drive, "c:/repo/src");
            assert_eq!(unc, "//server/share/src");
        } else {
            assert_eq!(drive, "C:/repo/src");
            assert_eq!(unc, "//server/share/src");
        }
        assert!(normalize_absolute_path(r"\\?\C:\repo\*.rs").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn runtime_path_authorization_defers_symlink_resolution_to_codex() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        fs::create_dir_all(workspace.join("owned")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let workspace_text = workspace.to_string_lossy().into_owned();
        let mut contract = worker_contract("worker_symlink", "owned");
        contract["root"] = json!(workspace_text);
        let input = contract_input("worker_symlink", "codey_worker", contract);
        pre_spawn_with_workspace(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&workspace_text),
            0,
            10,
        )
        .unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&json!({ "agent_id": "agent-symlink" })),
            20,
        )
        .unwrap();
        fs::remove_dir(workspace.join("owned")).unwrap();
        symlink(&outside, workspace.join("owned")).unwrap();

        let patch = json!({
            "patch": "*** Begin Patch\n*** Add File: owned/escape.rs\n+outside\n*** End Patch"
        });
        assert_eq!(
            authorize_child_tool_with_context(
                temp.path(),
                "runtime-a",
                "session-a",
                ChildToolContext {
                    agent_id: "agent-symlink",
                    agent_type: None,
                    transcript_path: None,
                    tool_name: "apply_patch",
                    tool_input: Some(&patch),
                },
                30,
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn stale_recovery_fences_the_ledger_before_markers_are_removed() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "research_stale",
            "codey_deep_research",
            research_contract("research_stale"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        assert_eq!(
            active_reservation_count(temp.path(), "runtime-a", "session-a", 20).unwrap(),
            Some(1)
        );
        assert_eq!(
            recover_active_reservations(
                temp.path(),
                "runtime-a",
                "session-a",
                "test recovery",
                30,
            )
            .unwrap(),
            1
        );
        assert_eq!(
            active_reservation_count(temp.path(), "runtime-a", "session-a", 40).unwrap(),
            Some(0)
        );
        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 50).unwrap().unwrap();
        let reservation = &ledger.reservations["research_stale"];
        assert_eq!(reservation.state, ReservationState::Recovered);
        assert_eq!(reservation.outcome, ExecutionOutcome::Lost);
        assert!(reservation.agent_id_hash.is_none());
        assert_eq!(reservation.fenced_at_ms, Some(30));
    }

    #[test]
    fn child_rule_changes_take_effect_without_process_restart() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "worker_hot_reload",
            "codey_worker",
            worker_contract("worker_hot_reload", "."),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&json!({ "agent_id": "agent-hot-reload" })),
            20,
        )
        .unwrap();

        assert_eq!(
            authorize_child_tool(
                temp.path(),
                "runtime-a",
                "session-a",
                "agent-hot-reload",
                "Bash",
                Some(&json!({ "command": "cargo test --lib" })),
                30,
            )
            .unwrap(),
            None
        );

        let mut live_rules = rules::embedded().clone();
        live_rules.revision = 2;
        live_rules.rules.push(rules::RuleDefinition {
            id: "deny-worker-command-hot".to_string(),
            priority: 950,
            effect: RuleEffect::Deny,
            actors: vec![RuleActor::Child],
            roles: vec!["codey_worker".to_string()],
            tools: Vec::new(),
            tool_classes: vec![ToolClass::Command],
            explanation: "测试热更新后禁止 worker 命令。".to_string(),
        });
        std::fs::write(
            rules::live_rule_path(temp.path()),
            serde_json::to_vec_pretty(&live_rules).unwrap(),
        )
        .unwrap();

        let denial = authorize_child_tool(
            temp.path(),
            "runtime-a",
            "session-a",
            "agent-hot-reload",
            "Bash",
            Some(&json!({ "command": "cargo test --lib" })),
            40,
        )
        .unwrap()
        .unwrap();
        assert!(denial.contains("deny-worker-command-hot"));
    }

    #[test]
    fn exact_successful_acceptance_command_clears_the_debt() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&json!({ "agent_id": "agent-a" })),
            20,
        )
        .unwrap();
        subagent_stopped(temp.path(), "runtime-a", "session-a", "agent-a", 30).unwrap();
        let command = json!({
            "command": "# codey-accept:worker_a:tests\ncargo test -p codey --lib"
        });
        assert_eq!(
            pre_root_tool(temp.path(), "runtime-a", "session-a", Some(&command), 40,).unwrap(),
            None
        );
        post_root_tool(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&command),
            Some(&json!({ "exit_code": 0, "output": "ok" })),
            50,
        )
        .unwrap();
        assert_eq!(
            pending_acceptance_reason(temp.path(), "runtime-a", "session-a", 60).unwrap(),
            None
        );
        commit_batch_decision_for_test(
            temp.path(),
            "session-a",
            1,
            RootBatchDecision::Complete,
            61,
        );
        settle_turn(temp.path(), "runtime-a", "session-a", 70).unwrap();
        assert!(
            !temp
                .path()
                .join(hash_component("session-a"))
                .join(LEDGER_FILE)
                .exists()
        );
    }

    #[test]
    fn acceptance_cannot_pass_before_the_worker_settles() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        let command = json!({
            "command": "# codey-accept:worker_a:tests\ncargo test -p codey --lib"
        });

        let denial = pre_root_tool(temp.path(), "runtime-a", "session-a", Some(&command), 20)
            .unwrap()
            .unwrap();
        assert!(denial.contains("尚未进入终态"));
        post_root_tool(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&command),
            Some(&json!({ "exit_code": 0, "output": "stale" })),
            30,
        )
        .unwrap();
        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 40).unwrap().unwrap();
        assert_eq!(
            ledger.reservations["worker_a"].acceptance[0].status,
            AcceptanceStatus::Pending
        );
        drop(ledger);
        drop(store);

        observe_status_response(temp.path(), "runtime-a", "session-a", None, true, 50).unwrap();
        assert_eq!(
            pre_root_tool(temp.path(), "runtime-a", "session-a", Some(&command), 60,).unwrap(),
            None
        );
        post_root_tool(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&command),
            Some(&json!({ "exit_code": 0, "output": "fresh" })),
            70,
        )
        .unwrap();
        assert_eq!(
            pending_acceptance_reason(temp.path(), "runtime-a", "session-a", 80).unwrap(),
            None
        );
    }

    #[test]
    fn failed_or_unstructured_acceptance_evidence_keeps_the_debt() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&input),
            Some(&json!({ "agent_id": "agent-a" })),
            20,
        )
        .unwrap();
        subagent_stopped(temp.path(), "runtime-a", "session-a", "agent-a", 30).unwrap();
        let command = json!({
            "command": "# codey-accept:worker_a:tests\ncargo test -p codey --lib"
        });

        pre_root_tool(temp.path(), "runtime-a", "session-a", Some(&command), 40).unwrap();
        post_root_tool(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&command),
            Some(&json!({ "exit_code": 1, "output": "failed" })),
            50,
        )
        .unwrap();
        assert!(
            pending_acceptance_reason(temp.path(), "runtime-a", "session-a", 60)
                .unwrap()
                .is_some()
        );

        pre_root_tool(temp.path(), "runtime-a", "session-a", Some(&command), 70).unwrap();
        post_root_tool(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&command),
            Some(&json!({ "output": "exit_code = 0" })),
            80,
        )
        .unwrap();
        assert!(
            pending_acceptance_reason(temp.path(), "runtime-a", "session-a", 90)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn acceptance_evidence_supports_safe_text_fallback_and_empty_error_fields() {
        assert_eq!(POST_TOOL_HOOK_MATCHER, "*");
        assert_eq!(
            classify_acceptance_evidence(Some(&json!({
                "exit_code": 0,
                "error": "",
                "output": "ok"
            }))),
            AcceptanceEvidence::Passed
        );
        assert_eq!(
            classify_acceptance_evidence(Some(&Value::String("exit code: 0".to_string()))),
            AcceptanceEvidence::Passed
        );
        assert_eq!(
            classify_acceptance_evidence(Some(&json!({ "output": "exit_code = 0" }))),
            AcceptanceEvidence::MissingExitStatus
        );
        assert_eq!(
            classify_acceptance_evidence(Some(&json!({
                "output": "{\"exit_code\":0}"
            }))),
            AcceptanceEvidence::MissingExitStatus
        );
        assert_eq!(
            classify_acceptance_evidence(Some(&Value::String(
                "test output\nexit code: 0".to_string()
            ))),
            AcceptanceEvidence::MissingExitStatus
        );
    }

    #[test]
    fn acceptance_debt_releases_after_three_failures_with_an_explicit_notice() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        observe_status_response(temp.path(), "runtime-a", "session-a", None, true, 15).unwrap();
        let command = json!({
            "command": "# codey-accept:worker_a:tests\ncargo test -p codey --lib"
        });
        for now_ms in [20, 30, 40] {
            post_root_tool(
                temp.path(),
                "runtime-a",
                "session-a",
                Some(&command),
                Some(&json!({ "exit_code": 1, "output": "failed" })),
                now_ms,
            )
            .unwrap();
        }

        let notice = pending_acceptance_reason(temp.path(), "runtime-a", "session-a", 50)
            .unwrap()
            .unwrap();
        assert!(notice.contains("没有被标记为通过"));
        assert!(notice.contains("失败 3 次"));
        assert_eq!(
            pending_acceptance_reason(temp.path(), "runtime-a", "session-a", 60).unwrap(),
            None
        );
        let complete = batch_decision_input(
            1,
            RootBatchDecision::Complete,
            "unverifiable-cannot-complete",
        );
        let denial = prepare_batch_decision(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&complete),
            0,
            60,
        )
        .unwrap()
        .unwrap();
        assert!(denial.contains("不能提交 `complete`"));
        commit_batch_decision_for_test(temp.path(), "session-a", 1, RootBatchDecision::Blocked, 61);
        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 65).unwrap().unwrap();
        store
            .write_settlement_receipt(&ledger, "runtime-a", "session-a")
            .unwrap();
        store
            .write_settlement_receipt(&ledger, "runtime-a", "session-a")
            .unwrap();
        drop(ledger);
        drop(store);
        settle_turn(temp.path(), "runtime-a", "session-a", 70).unwrap();
        let session_dir = temp.path().join(hash_component("session-a"));
        let receipt_paths = fs::read_dir(&session_dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(SETTLEMENT_RECEIPT_PREFIX))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            receipt_paths.len(),
            1,
            "settlement receipt must be idempotent"
        );
        let receipt_path = &receipt_paths[0];
        let receipt: Value = serde_json::from_slice(&fs::read(receipt_path).unwrap()).unwrap();
        assert_eq!(receipt["final_decision"], "blocked");
        assert_eq!(receipt["unverifiable_acceptance"][0]["task_id"], "worker_a");
        assert_eq!(receipt["unverifiable_acceptance"][0]["check_id"], "tests");
    }

    #[test]
    fn unchanged_acceptance_debt_releases_after_three_stop_observations() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();

        for now_ms in [20, 30] {
            let reason = pending_acceptance_reason(temp.path(), "runtime-a", "session-a", now_ms)
                .unwrap()
                .unwrap();
            assert!(reason.contains("codey-accept:worker_a:tests"));
        }
        let notice = pending_acceptance_reason(temp.path(), "runtime-a", "session-a", 40)
            .unwrap()
            .unwrap();
        assert!(notice.contains("连续 3 次 Stop 未取得新证据"));
        assert_eq!(
            pending_acceptance_reason(temp.path(), "runtime-a", "session-a", 50).unwrap(),
            None
        );
    }

    #[test]
    fn acceptance_debt_releases_after_the_stall_grace_even_before_three_stops() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();

        assert!(
            pending_acceptance_reason(temp.path(), "runtime-a", "session-a", 20)
                .unwrap()
                .unwrap()
                .contains("codey-accept:worker_a:tests")
        );
        let notice = pending_acceptance_reason(
            temp.path(),
            "runtime-a",
            "session-a",
            20 + ACCEPTANCE_STALL_GRACE_MILLIS,
        )
        .unwrap()
        .unwrap();
        assert!(notice.contains("持续 10 分钟未取得新证据"));
        assert_eq!(
            pending_acceptance_reason(
                temp.path(),
                "runtime-a",
                "session-a",
                21 + ACCEPTANCE_STALL_GRACE_MILLIS,
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn corrupted_ledger_fails_closed_without_overwriting_evidence() {
        let temp = tempfile::tempdir().unwrap();
        let ledger_path = temp
            .path()
            .join(hash_component("session-a"))
            .join(LEDGER_FILE);
        std::fs::create_dir_all(ledger_path.parent().unwrap()).unwrap();
        std::fs::write(&ledger_path, b"{not-valid-json").unwrap();
        let input = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/a.rs"),
        );

        let error =
            pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap_err();
        assert!(format!("{error:#}").contains("解析 Codey 子代理编排账本失败"));
        assert_eq!(std::fs::read(&ledger_path).unwrap(), b"{not-valid-json");
    }

    #[test]
    fn persisted_duplicate_agent_bindings_are_rejected_on_load() {
        let temp = tempfile::tempdir().unwrap();
        for (index, task) in ["research_a", "research_b"].into_iter().enumerate() {
            let input = contract_input(task, "codey_deep_research", research_contract(task));
            pre_spawn(
                temp.path(),
                "runtime-a",
                "session-a",
                Some(&input),
                index,
                10 + index as u64,
            )
            .unwrap();
        }
        let ledger_path = temp
            .path()
            .join(hash_component("session-a"))
            .join(LEDGER_FILE);
        let mut ledger: Value = serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
        for task in ["research_a", "research_b"] {
            ledger["reservations"][task]["agent_id_hash"] = json!("same-hash");
        }
        fs::write(&ledger_path, serde_json::to_vec(&ledger).unwrap()).unwrap();

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let error = store.load("runtime-a", "session-a", 30).unwrap_err();
        assert!(format!("{error:#}").contains(AGENT_ID_COLLISION_ERROR_CODE));
    }

    #[test]
    fn schema_v2_ledgers_are_atomically_migrated_to_batch_state() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "research_a",
            "codey_deep_research",
            research_contract("research_a"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();

        let ledger_path = temp
            .path()
            .join(hash_component("session-a"))
            .join(LEDGER_FILE);
        let mut legacy: Value =
            serde_json::from_slice(&std::fs::read(&ledger_path).unwrap()).unwrap();
        let object = legacy.as_object_mut().unwrap();
        object.insert("schema_version".to_string(), json!(2));
        object.insert("point_limit".to_string(), json!(8));
        object.insert("attempt_limit".to_string(), json!(4));
        object.insert("points_spent".to_string(), json!(1));
        object.insert("spawn_attempts".to_string(), json!(1));
        object.insert("total_spawn_attempts".to_string(), json!(1));
        object.remove("batch_number");
        object.remove("issued_task_ids");
        object.remove("next_fencing_token");
        let reservation = object["reservations"].as_object_mut().unwrap()["research_a"]
            .as_object_mut()
            .unwrap();
        reservation.insert("cost_points".to_string(), json!(1));
        reservation.remove("batch_number");
        for field in [
            "outcome",
            "deadline_at_ms",
            "attempt_id",
            "fencing_token",
            "policy_revision",
            "fenced_at_ms",
            "spawn_failed",
            "input_schema",
            "output_schema",
        ] {
            reservation.remove(field);
        }
        std::fs::write(&ledger_path, serde_json::to_vec(&legacy).unwrap()).unwrap();

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 20).unwrap().unwrap();
        assert_eq!(ledger.schema_version, LEDGER_SCHEMA_VERSION);
        assert_eq!(ledger.batch_number, 1);
        assert!(ledger.issued_task_ids.contains("research_a"));
        assert_eq!(ledger.reservations["research_a"].batch_number, 1);
        assert_eq!(
            ledger.reservations["research_a"].outcome,
            ExecutionOutcome::Unknown
        );
        assert!(!ledger.reservations["research_a"].attempt_id.is_empty());
        assert!(ledger.reservations["research_a"].fencing_token > 0);
        drop(ledger);
        drop(store);

        let migrated: Value =
            serde_json::from_slice(&std::fs::read(&ledger_path).unwrap()).unwrap();
        assert_eq!(migrated["schema_version"], json!(LEDGER_SCHEMA_VERSION));
        assert_eq!(migrated["batch_number"], json!(1));
        assert_eq!(migrated["issued_task_ids"], json!(["research_a"]));
        for retired in [
            "point_limit",
            "attempt_limit",
            "points_spent",
            "spawn_attempts",
            "total_spawn_attempts",
        ] {
            assert!(migrated.get(retired).is_none());
        }
        assert!(
            migrated["reservations"]["research_a"]
                .get("cost_points")
                .is_none()
        );
    }

    #[test]
    fn runtime_recovery_keeps_unpaid_write_acceptance_only() {
        let temp = tempfile::tempdir().unwrap();
        let write = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );
        let read = contract_input(
            "research_a",
            "codey_deep_research",
            research_contract("research_a"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&write), 0, 10).unwrap();
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&read), 0, 20).unwrap();
        let reason = pending_acceptance_reason(temp.path(), "runtime-b", "session-a", 30)
            .unwrap()
            .unwrap();
        assert!(reason.contains("worker_a"));
        assert!(!reason.contains("research_a"));
        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-b", "session-a", 40).unwrap().unwrap();
        assert_eq!(ledger.reservations.len(), 1);
        assert_eq!(
            ledger.reservations["worker_a"].state,
            ReservationState::Recovered
        );
    }

    #[test]
    fn runtime_recovery_drops_failed_write_spawns_without_creating_acceptance_debt() {
        let temp = tempfile::tempdir().unwrap();
        let write = contract_input(
            "worker_failed",
            "codey_worker",
            worker_contract("worker_failed", "backend/src"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&write), 0, 10).unwrap();
        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&write),
            Some(&json!({ "isError": true, "error": "capacity" })),
            20,
        )
        .unwrap();

        assert_eq!(
            pending_acceptance_reason(temp.path(), "runtime-b", "session-a", 30).unwrap(),
            None
        );
        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-b", "session-a", 40).unwrap().unwrap();
        assert!(ledger.reservations.is_empty());
        assert!(ledger.issued_task_ids.is_empty());
    }

    #[test]
    fn session_end_fences_and_preserves_current_runtime_outstanding_work() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
        let ledger_path = temp
            .path()
            .join(hash_component("session-a"))
            .join(LEDGER_FILE);
        assert!(ledger_path.exists());

        end_session(temp.path(), "runtime-a", "session-a", 40).unwrap();
        assert!(ledger_path.exists());
        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 50).unwrap().unwrap();
        let reservation = &ledger.reservations["worker_a"];
        assert_eq!(reservation.state, ReservationState::Recovered);
        assert_eq!(reservation.outcome, ExecutionOutcome::Lost);
        assert_eq!(reservation.fenced_at_ms, Some(40));
        assert!(reservation.agent_id_hash.is_none());
    }

    #[test]
    fn session_end_keeps_foreign_runtime_ledgers_with_outstanding_work() {
        let temp = tempfile::tempdir().unwrap();
        let write = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );
        let ledger_path = temp
            .path()
            .join(hash_component("session-a"))
            .join(LEDGER_FILE);

        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&write), 0, 10).unwrap();
        end_session(temp.path(), "runtime-b", "session-a", 40).unwrap();
        assert!(ledger_path.exists());

        post_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&write),
            Some(&json!({ "agent_id": "agent-a" })),
            20,
        )
        .unwrap();
        observe_status_response(temp.path(), "runtime-a", "session-a", None, true, 30).unwrap();
        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 40).unwrap().unwrap();
        assert_eq!(
            ledger.reservations["worker_a"].state,
            ReservationState::Terminal
        );
        drop(ledger);
        drop(store);

        // 写入角色的机械验收债仍未清偿，即使代次不一致也不能删除账本。
        end_session(temp.path(), "runtime-b", "session-a", 40).unwrap();
        assert!(ledger_path.exists());
    }

    #[test]
    fn session_end_removes_foreign_runtime_ledgers_without_outstanding_work() {
        let temp = tempfile::tempdir().unwrap();
        let read = contract_input(
            "research_a",
            "codey_deep_research",
            research_contract("research_a"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&read), 0, 10).unwrap();
        observe_status_response(temp.path(), "runtime-a", "session-a", None, true, 20).unwrap();
        let ledger_path = temp
            .path()
            .join(hash_component("session-a"))
            .join(LEDGER_FILE);
        assert!(ledger_path.exists());

        end_session(temp.path(), "runtime-b", "session-a", 40).unwrap();
        assert!(!ledger_path.exists());
    }

    #[test]
    fn session_end_quarantines_unreadable_ledgers() {
        let temp = tempfile::tempdir().unwrap();
        end_session(temp.path(), "runtime-a", "session-a", 40).unwrap();

        let ledger_path = temp
            .path()
            .join(hash_component("session-a"))
            .join(LEDGER_FILE);
        std::fs::create_dir_all(ledger_path.parent().unwrap()).unwrap();
        std::fs::write(&ledger_path, b"{not-valid-json").unwrap();
        end_session(temp.path(), "runtime-a", "session-a", 40).unwrap();
        assert!(!ledger_path.exists());
        let evidence = std::fs::read_dir(ledger_path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("orchestrator-ledger-v1.corrupt-")
            })
            .expect("损坏账本应被隔离并保留证据");
        assert_eq!(std::fs::read(evidence.path()).unwrap(), b"{not-valid-json");
    }
}
