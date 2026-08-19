use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::subagent_policy::{RoleAccess, RolePolicy, role_policy};

pub(crate) const CONTRACT_PREFIX: &str = "CODEY_DELEGATION_V2=";
const LEGACY_CONTRACT_PREFIX_V1: &str = "CODEY_DELEGATION_V1=";
pub(crate) const POST_TOOL_HOOK_MATCHER: &str = "*";

const LEDGER_SCHEMA_VERSION: u32 = 2;
const MIN_LEDGER_SCHEMA_VERSION: u32 = 1;
const LEDGER_FILE: &str = "orchestrator-ledger-v1.json";
const LEDGER_LOCK_FILE: &str = "orchestrator-ledger-v1.lock";
const DEFAULT_POINT_LIMIT: u16 = 8;
const MAX_POINT_LIMIT: u16 = 12;
const DEFAULT_ATTEMPT_LIMIT: u16 = 4;
const MAX_ATTEMPT_LIMIT: u16 = 6;
const MAX_CLAIMS_PER_MODE: usize = 16;
const MAX_ACCEPTANCE_CHECKS: usize = 3;
const MAX_ACCEPTANCE_COMMAND_CHARS: usize = 1024;
const MAX_CONTRACT_LINE_CHARS: usize = 8 * 1024;
const MAX_ACCEPTANCE_FAILURES: u16 = 3;
const MAX_UNCHANGED_ACCEPTANCE_STOPS: u16 = 3;
const ACCEPTANCE_STALL_GRACE_MILLIS: u64 = 10 * 60 * 1000;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DelegationContract {
    id: String,
    #[serde(rename = "why")]
    reason: String,
    #[serde(default)]
    branch_calls: Vec<u16>,
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
    write_paths: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SessionLedger {
    schema_version: u32,
    runtime_id_hash: String,
    session_id_hash: String,
    revision: u64,
    created_at_ms: u64,
    updated_at_ms: u64,
    point_limit: u16,
    attempt_limit: u16,
    points_spent: u16,
    spawn_attempts: u16,
    reservations: BTreeMap<String, Reservation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Reservation {
    task_id: String,
    role: String,
    cost_points: u16,
    write_capable: bool,
    visual: bool,
    workspace_root: Option<String>,
    read_paths: Vec<String>,
    write_paths: Vec<String>,
    state: ReservationState,
    agent_id_hash: Option<String>,
    created_at_ms: u64,
    updated_at_ms: u64,
    acceptance: Vec<AcceptanceEntry>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReservationState {
    Pending,
    Running,
    Terminal,
    Failed,
    Recovered,
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AcceptanceStatus {
    Pending,
    Passed,
    Failed,
    Unverifiable,
}

struct LedgerStore {
    lock: File,
    ledger_path: PathBuf,
}

impl LedgerStore {
    fn open(state_root: &Path, session_id: &str) -> Result<Self> {
        fs::create_dir_all(state_root).with_context(|| {
            format!(
                "创建 Codey 子代理编排状态目录失败：{}",
                state_root.display()
            )
        })?;
        let lock_path = state_root.join(LEDGER_LOCK_FILE);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("打开 Codey 子代理预算账本锁失败：{}", lock_path.display()))?;
        lock.lock_exclusive()
            .with_context(|| format!("获取 Codey 子代理预算账本锁失败：{}", lock_path.display()))?;
        let session_dir = state_root.join(hash_component(session_id));
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
                        "读取 Codey 子代理预算账本失败：{}",
                        self.ledger_path.display()
                    )
                });
            }
        };
        let mut ledger: SessionLedger = serde_json::from_slice(&bytes).with_context(|| {
            format!(
                "解析 Codey 子代理预算账本失败：{}",
                self.ledger_path.display()
            )
        })?;
        anyhow::ensure!(
            (MIN_LEDGER_SCHEMA_VERSION..=LEDGER_SCHEMA_VERSION).contains(&ledger.schema_version),
            "Codey 子代理预算账本版本不受支持：{}",
            ledger.schema_version
        );
        ledger.schema_version = LEDGER_SCHEMA_VERSION;
        let session_id_hash = hash_component(session_id);
        anyhow::ensure!(
            ledger.session_id_hash == session_id_hash,
            "Codey 子代理预算账本会话标识不一致"
        );
        let runtime_id_hash = hash_component(runtime_id);
        if ledger.runtime_id_hash != runtime_id_hash {
            ledger.reservations.retain(|_, reservation| {
                reservation.write_capable && reservation_has_pending_acceptance(reservation)
            });
            for reservation in ledger.reservations.values_mut() {
                reservation.state = ReservationState::Recovered;
                reservation.agent_id_hash = None;
                reservation.updated_at_ms = now_ms;
            }
            ledger.runtime_id_hash = runtime_id_hash;
            ledger.points_spent = ledger.reservations.values().fold(0, |total, reservation| {
                total.saturating_add(reservation.cost_points)
            });
            ledger.spawn_attempts = u16::try_from(ledger.reservations.len()).unwrap_or(u16::MAX);
            ledger.updated_at_ms = now_ms;
        }
        Ok(Some(ledger))
    }

    fn save(&self, ledger: &mut SessionLedger, now_ms: u64) -> Result<()> {
        let parent = self
            .ledger_path
            .parent()
            .context("Codey 子代理预算账本缺少父目录")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("创建 Codey 子代理预算账本目录失败：{}", parent.display()))?;
        ledger.revision = ledger.revision.saturating_add(1);
        ledger.updated_at_ms = now_ms;
        let bytes = serde_json::to_vec(ledger).context("序列化 Codey 子代理预算账本失败")?;
        crate::fs_util::atomic_write(&self.ledger_path, &bytes).with_context(|| {
            format!(
                "原子替换 Codey 子代理预算账本失败：{}",
                self.ledger_path.display()
            )
        })
    }

    fn remove(&self) -> Result<()> {
        match fs::remove_file(&self.ledger_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "删除 Codey 子代理预算账本失败：{}",
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
                        format!("清理 Codey 子代理预算账本目录失败：{}", parent.display())
                    });
                }
            }
        }
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
            point_limit: DEFAULT_POINT_LIMIT,
            attempt_limit: DEFAULT_ATTEMPT_LIMIT,
            points_spent: 0,
            spawn_attempts: 0,
            reservations: BTreeMap::new(),
        }
    }
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
    pre_spawn_with_workspace(
        state_root,
        runtime_id,
        session_id,
        tool_input,
        None,
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
    let prepared = match prepare_contract_with_workspace(tool_input, hook_workspace_root) {
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
    if ledger.reservations.contains_key(&prepared.contract.id) {
        return Ok(Some(format!(
            "Codey 自适应委派门禁：任务 ID `{}` 已在本轮预算账本中，禁止重复派生；失败重试必须使用新的任务 ID。",
            prepared.contract.id
        )));
    }
    if let Some(conflict) = resource_conflict(&prepared, &ledger) {
        return Ok(Some(conflict));
    }

    let observed_task_count = ledger.reservations.len().saturating_add(1);
    let (desired_points, desired_attempts) =
        adaptive_limits(&prepared.contract, observed_task_count);
    ledger.point_limit = ledger.point_limit.max(desired_points).min(MAX_POINT_LIMIT);
    ledger.attempt_limit = ledger
        .attempt_limit
        .max(desired_attempts)
        .min(MAX_ATTEMPT_LIMIT);
    if ledger.spawn_attempts >= ledger.attempt_limit {
        return Ok(Some(format!(
            "Codey 可恢复预算门禁：本轮已使用 {} 次派生尝试，达到上限 {}；请由主代理接管剩余工作。",
            ledger.spawn_attempts, ledger.attempt_limit
        )));
    }
    let cost_points = u16::from(prepared.policy.cost_points);
    if ledger.points_spent.saturating_add(cost_points) > ledger.point_limit {
        return Ok(Some(format!(
            "Codey 可恢复预算门禁：任务 `{}` 需要 {} 点，当前已用 {}/{} 点；请缩小范围、改用更轻角色或由主代理直接处理。",
            prepared.contract.id, cost_points, ledger.points_spent, ledger.point_limit
        )));
    }

    ledger.spawn_attempts = ledger.spawn_attempts.saturating_add(1);
    ledger.points_spent = ledger.points_spent.saturating_add(cost_points);
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
    ledger.reservations.insert(
        prepared.contract.id.clone(),
        Reservation {
            task_id: prepared.contract.id,
            role: prepared.role,
            cost_points,
            write_capable: prepared.policy.access == RoleAccess::Write,
            visual: prepared.policy.visual,
            workspace_root: prepared.workspace_root,
            read_paths: prepared.read_paths,
            write_paths: prepared.write_paths,
            state: ReservationState::Pending,
            agent_id_hash: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
            acceptance,
        },
    );
    store.save(&mut ledger, now_ms)?;
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
    let Some(reservation) = ledger.reservations.get_mut(task_id) else {
        return Ok(());
    };
    if tool_response.is_some_and(response_is_explicit_failure) {
        let cost = reservation.cost_points;
        reservation.state = ReservationState::Failed;
        reservation.updated_at_ms = now_ms;
        ledger.points_spent = ledger.points_spent.saturating_sub(cost);
        ledger.reservations.remove(task_id);
    } else {
        reservation.state = ReservationState::Running;
        reservation.updated_at_ms = now_ms;
        if let Some(agent_id) = tool_response.and_then(extract_agent_identifier) {
            reservation.agent_id_hash = Some(hash_component(agent_id));
        }
    }
    store.save(&mut ledger, now_ms)
}

pub(crate) fn subagent_started(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    agent_id: &str,
    now_ms: u64,
) -> Result<()> {
    update_reservation_lifecycle(
        state_root,
        runtime_id,
        session_id,
        agent_id,
        ReservationState::Running,
        now_ms,
    )
}

pub(crate) fn subagent_stopped(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    agent_id: &str,
    now_ms: u64,
) -> Result<()> {
    update_reservation_lifecycle(
        state_root,
        runtime_id,
        session_id,
        agent_id,
        ReservationState::Terminal,
        now_ms,
    )
}

fn update_reservation_lifecycle(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    agent_id: &str,
    state: ReservationState,
    now_ms: u64,
) -> Result<()> {
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(mut ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(());
    };
    let agent_hash = hash_component(agent_id);
    let exact_task = ledger
        .reservations
        .keys()
        .find(|task_id| identifier_mentions_task(agent_id, task_id))
        .cloned();
    let bound_task = ledger
        .reservations
        .iter()
        .find_map(|(task_id, reservation)| {
            (reservation.agent_id_hash.as_deref() == Some(agent_hash.as_str()))
                .then(|| task_id.clone())
        });
    let fallback_task = {
        let candidates = ledger
            .reservations
            .iter()
            .filter(|(_, reservation)| {
                reservation.agent_id_hash.is_none()
                    && !matches!(
                        reservation.state,
                        ReservationState::Terminal | ReservationState::Failed
                    )
            })
            .map(|(task_id, _)| task_id.clone())
            .collect::<Vec<_>>();
        (candidates.len() == 1).then(|| candidates[0].clone())
    };
    let Some(task_id) = bound_task.or(exact_task).or(fallback_task) else {
        return Ok(());
    };
    if let Some(reservation) = ledger.reservations.get_mut(&task_id) {
        reservation.state = state;
        reservation.agent_id_hash = Some(agent_hash);
        reservation.updated_at_ms = now_ms;
    }
    store.save(&mut ledger, now_ms)
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
    let mut terminal_tasks = BTreeSet::new();
    if let Some(response) = tool_response {
        collect_terminal_task_ids(response, &ledger, &mut terminal_tasks);
    }
    if all_terminal {
        terminal_tasks.extend(ledger.reservations.keys().cloned());
    }
    let mut changed = false;
    for task_id in terminal_tasks {
        if let Some(reservation) = ledger.reservations.get_mut(&task_id)
            && reservation.state != ReservationState::Failed
        {
            reservation.state = ReservationState::Terminal;
            reservation.updated_at_ms = now_ms;
            changed = true;
        }
    }
    if changed {
        store.save(&mut ledger, now_ms)?;
    }
    Ok(())
}

pub(crate) fn authorize_child_tool(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    agent_id: &str,
    tool_name: &str,
    tool_input: Option<&Value>,
    now_ms: u64,
) -> Result<Option<String>> {
    if !known_write_tool(tool_name) {
        return Ok(None);
    }
    let observed_paths = extract_write_paths(tool_name, tool_input);
    if observed_paths.is_empty() {
        return Ok(Some(format!(
            "Codey 能力/资源门禁：无法从写入工具 `{tool_name}` 的输入中确定目标路径；请改用能显式报告路径的 apply_patch/FastCtx replace，或把任务交回主代理。"
        )));
    }
    let store = LedgerStore::open(state_root, session_id)?;
    let Some(mut ledger) = store.load(runtime_id, session_id, now_ms)? else {
        return Ok(Some(
            "Codey 能力/资源门禁：当前子代理没有可恢复的委派契约，禁止执行写入工具。".to_string(),
        ));
    };
    let agent_hash = hash_component(agent_id);
    let bound = ledger
        .reservations
        .iter()
        .find_map(|(task_id, reservation)| {
            (reservation.agent_id_hash.as_deref() == Some(agent_hash.as_str()))
                .then(|| task_id.clone())
        });
    let candidates = ledger
        .reservations
        .iter()
        .filter(|(_, reservation)| {
            reservation.write_capable
                && !matches!(
                    reservation.state,
                    ReservationState::Terminal | ReservationState::Failed
                )
                && observed_paths
                    .iter()
                    .all(|path| reservation_covers_path(reservation, path))
        })
        .map(|(task_id, _)| task_id.clone())
        .collect::<Vec<_>>();
    let task_id = if let Some(bound) = bound {
        if candidates.iter().any(|candidate| candidate == &bound) {
            Some(bound)
        } else {
            None
        }
    } else if candidates.len() == 1 {
        Some(candidates[0].clone())
    } else {
        None
    };
    let Some(task_id) = task_id else {
        return Ok(Some(format!(
            "Codey 能力/资源门禁：子代理 `{agent_id}` 对目标路径没有唯一且有效的写入 ownership；禁止越界修改。"
        )));
    };
    if let Some(reservation) = ledger.reservations.get_mut(&task_id) {
        reservation.agent_id_hash = Some(agent_hash);
        reservation.updated_at_ms = now_ms;
    }
    store.save(&mut ledger, now_ms)?;
    Ok(None)
}

pub(crate) fn pre_root_tool(
    state_root: &Path,
    runtime_id: &str,
    session_id: &str,
    tool_input: Option<&Value>,
    now_ms: u64,
) -> Result<Option<String>> {
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
        return Ok(Some("Codey 机械验收门禁：找不到对应预算账本。".to_string()));
    };
    let Some(reservation) = ledger.reservations.get(task_id) else {
        return Ok(Some(format!(
            "Codey 机械验收门禁：不存在任务 `{task_id}`。"
        )));
    };
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
        if reservation.write_capable && reservation.state != ReservationState::Failed {
            if !matches!(
                reservation.state,
                ReservationState::Terminal | ReservationState::Recovered
            ) {
                reservation.state = ReservationState::Terminal;
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
    store.remove()
}

pub(crate) fn end_session(state_root: &Path, session_id: &str) -> Result<()> {
    LedgerStore::open(state_root, session_id)?.remove()
}

#[cfg(test)]
fn prepare_contract(tool_input: Option<&Value>) -> std::result::Result<PreparedContract, String> {
    prepare_contract_with_workspace(tool_input, None)
}

fn prepare_contract_with_workspace(
    tool_input: Option<&Value>,
    hook_workspace_root: Option<&str>,
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
    if string_field(input, &["fork_turns", "forkTurns"]) != Some("none") {
        return Err(contract_error("fork_turns 必须为 none"));
    }
    let policy = role_policy(role)
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
        for retired in ["calls", "files", "dirs", "large", "risk"] {
            values.remove(retired);
        }
    }
    let contract: DelegationContract = serde_json::from_value(contract_value)
        .map_err(|error| contract_error(&format!("契约 JSON 无效：{error}")))?;
    validate_task_id(&contract.id)?;
    if contract.id != task_name {
        return Err(contract_error("契约 id 必须与 task_name 完全一致"));
    }
    validate_delegation_reason_role(&contract.reason, role)?;
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
        return Err(contract_error("checks 最多 3 项"));
    }
    let workspace_root = contract
        .workspace_root
        .as_deref()
        .map(normalize_absolute_path)
        .transpose()
        .map_err(|error| contract_error(&format!("root 无效：{error}")))?;
    let read_paths = normalize_claims(&contract.read_paths, workspace_root.as_deref())?;
    let write_paths = normalize_claims(&contract.write_paths, workspace_root.as_deref())?;
    match policy.access {
        RoleAccess::ReadOnly => {
            if !write_paths.is_empty() || !contract.acceptance.is_empty() {
                return Err(contract_error("只读角色不能声明 write 或 checks"));
            }
        }
        RoleAccess::Write => {
            if write_paths.is_empty() {
                return Err(contract_error("写入角色必须声明至少一个 write ownership"));
            }
            if contract.acceptance.is_empty() {
                return Err(contract_error("写入角色必须声明至少一个机械 checks"));
            }
            if workspace_root.is_none() {
                return Err(contract_error("写入角色必须声明绝对 root"));
            }
        }
    }
    let mut check_ids = BTreeSet::new();
    for check in &contract.acceptance {
        validate_task_id(&check.id)?;
        if !check_ids.insert(check.id.as_str()) {
            return Err(contract_error("checks id 不能重复"));
        }
        let command = check.command.trim();
        if command.is_empty() || command.chars().count() > MAX_ACCEPTANCE_COMMAND_CHARS {
            return Err(contract_error("checks cmd 必须为 1..=1024 个字符"));
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
        contract,
        role: role.to_string(),
        policy,
        workspace_root,
        read_paths,
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
    let workspace_root = hook_workspace_root
        .map(normalize_absolute_path)
        .transpose()
        .map_err(|error| contract_error(&format!("Hook 工作目录无效：{error}")))?;
    if policy.access == RoleAccess::Write && workspace_root.is_none() {
        return Err(contract_error(
            "message 已由上游加密，且 Hook 未提供绝对工作目录，无法为写入角色建立保守 ownership",
        ));
    }
    let workspace_claims = workspace_root.iter().cloned().collect::<Vec<_>>();
    let (read_paths, write_paths) = match policy.access {
        RoleAccess::ReadOnly => (workspace_claims, Vec::new()),
        RoleAccess::Write => (Vec::new(), workspace_claims),
    };
    let contract = DelegationContract {
        id: task_name.to_string(),
        reason: "encrypted_message".to_string(),
        branch_calls: Vec::new(),
        visual: policy.visual,
        workspace_root: workspace_root.clone(),
        read_paths: read_paths.clone(),
        write_paths: write_paths.clone(),
        acceptance: Vec::new(),
    };
    Ok(PreparedContract {
        contract,
        role: role.to_string(),
        policy,
        workspace_root,
        read_paths,
        write_paths,
    })
}

fn contract_error(detail: &str) -> String {
    format!(
        "Codey 自适应委派门禁：{detail}。请在 message 最后一行追加紧凑契约，例如：{CONTRACT_PREFIX}{{\"id\":\"scan_auth\",\"why\":\"breadth\",\"visual\":false,\"read\":[],\"write\":[],\"checks\":[]}}"
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

fn validate_delegation_reason_role(reason: &str, role: &str) -> std::result::Result<(), String> {
    use crate::config::{
        SUBAGENT_ROLE_DEEP_RESEARCH, SUBAGENT_ROLE_DEFAULT, SUBAGENT_ROLE_QUICK_SCAN,
        SUBAGENT_ROLE_VISUAL_ANALYSIS, SUBAGENT_ROLE_VISUAL_WORKER, SUBAGENT_ROLE_WORKER,
    };

    let role_compatible = match reason {
        "multi_lookup" => role == SUBAGENT_ROLE_QUICK_SCAN,
        "breadth" | "context" => matches!(
            role,
            SUBAGENT_ROLE_DEEP_RESEARCH | SUBAGENT_ROLE_VISUAL_ANALYSIS | SUBAGENT_ROLE_DEFAULT
        ),
        "independent_work" => {
            matches!(role, SUBAGENT_ROLE_WORKER | SUBAGENT_ROLE_VISUAL_WORKER)
        }
        "parallel" | "high_risk" | "user_requested" => true,
        _ => {
            return Err(contract_error(
                "why 无效；允许 multi_lookup/parallel/breadth/context/independent_work/high_risk/user_requested",
            ));
        }
    };
    if role_compatible {
        Ok(())
    } else {
        Err(contract_error(
            "why 与 agent_type 不兼容；parallel 的 branch_calls 只用于自适应预算提示，不是任务规模硬门槛",
        ))
    }
}

fn adaptive_limits(contract: &DelegationContract, observed_task_count: usize) -> (u16, u16) {
    // Recent Codex runtimes can encrypt the complete spawn message before the
    // hook observes it. In that mode branch_calls is intentionally opaque, so
    // derive a conservative branch count from distinct reservations that this
    // ledger has actually admitted. Failed spawns are removed from the map and
    // therefore cannot inflate the budget through retries.
    let branches = if contract.reason == "parallel" {
        u16::try_from(contract.branch_calls.len()).unwrap_or(u16::MAX)
    } else if contract.reason == "encrypted_message" {
        u16::try_from(observed_task_count).unwrap_or(u16::MAX)
    } else {
        0
    };
    let adaptive = matches!(contract.reason.as_str(), "parallel" | "encrypted_message");
    let point_limit = if adaptive {
        DEFAULT_POINT_LIMIT
            .max(branches.saturating_mul(3))
            .min(MAX_POINT_LIMIT)
    } else {
        DEFAULT_POINT_LIMIT
    };
    let attempt_limit = if adaptive {
        DEFAULT_ATTEMPT_LIMIT
            .max(branches.saturating_add(1))
            .min(MAX_ATTEMPT_LIMIT)
    } else {
        DEFAULT_ATTEMPT_LIMIT
    };
    (point_limit, attempt_limit)
}

fn normalize_claims(
    claims: &[String],
    workspace_root: Option<&str>,
) -> std::result::Result<Vec<String>, String> {
    let mut normalized = BTreeSet::new();
    for claim in claims {
        let path = if is_absolute_path(claim) {
            normalize_absolute_path(claim)
        } else if let Some(root) = workspace_root {
            normalize_absolute_path(&format!("{}/{}", root.trim_end_matches('/'), claim))
        } else {
            Err("相对资源路径需要绝对 root".to_string())
        }
        .map_err(|error| contract_error(&format!("资源路径 `{claim}` 无效：{error}")))?;
        normalized.insert(path);
    }
    Ok(normalized.into_iter().collect())
}

fn normalize_absolute_path(value: &str) -> std::result::Result<String, String> {
    let replaced = value.trim().replace('\\', "/");
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
        reservation.state != ReservationState::Failed
            && (reservation.state != ReservationState::Terminal
                || reservation_has_pending_acceptance(reservation))
    }) {
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

fn reservation_covers_path(reservation: &Reservation, observed: &str) -> bool {
    let normalized = if is_absolute_path(observed) {
        normalize_absolute_path(observed).ok()
    } else {
        reservation.workspace_root.as_deref().and_then(|root| {
            normalize_absolute_path(&format!("{}/{}", root.trim_end_matches('/'), observed)).ok()
        })
    };
    normalized.is_some_and(|path| {
        reservation
            .write_paths
            .iter()
            .any(|claim| path_is_within(&path, claim))
    })
}

fn known_write_tool(tool_name: &str) -> bool {
    matches!(
        normalized_tool_name(tool_name).as_str(),
        "apply_patch" | "replace" | "write_file" | "edit_file" | "create_file" | "delete_file"
    )
}

fn normalized_tool_name(tool_name: &str) -> String {
    tool_name
        .split(['.', '/', ':'])
        .next_back()
        .unwrap_or(tool_name)
        .trim_start_matches('_')
        .to_ascii_lowercase()
}

fn extract_write_paths(tool_name: &str, tool_input: Option<&Value>) -> Vec<String> {
    let Some(input) = tool_input else {
        return Vec::new();
    };
    let mut paths = BTreeSet::new();
    if normalized_tool_name(tool_name) == "apply_patch" {
        collect_patch_paths(input, &mut paths);
    } else {
        collect_path_fields(input, &mut paths);
    }
    paths.into_iter().collect()
}

fn collect_patch_paths(value: &Value, paths: &mut BTreeSet<String>) {
    match value {
        Value::String(patch) => {
            for line in patch.lines() {
                for prefix in [
                    "*** Add File: ",
                    "*** Update File: ",
                    "*** Delete File: ",
                    "*** Move to: ",
                ] {
                    if let Some(path) = line.strip_prefix(prefix) {
                        let path = path.trim();
                        if !path.is_empty() {
                            paths.insert(path.to_string());
                        }
                    }
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_patch_paths(value, paths);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_patch_paths(value, paths);
            }
        }
        _ => {}
    }
}

fn collect_path_fields(value: &Value, paths: &mut BTreeSet<String>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_path_fields(value, paths);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                let key = normalized_identifier(key);
                if matches!(key.as_str(), "path" | "filepath" | "targetpath") {
                    if let Some(path) = value.as_str() {
                        paths.insert(path.to_string());
                    }
                } else if matches!(key.as_str(), "paths" | "files") {
                    if let Some(values) = value.as_array() {
                        for value in values {
                            if let Some(path) = value.as_str() {
                                paths.insert(path.to_string());
                            }
                        }
                    }
                } else {
                    collect_path_fields(value, paths);
                }
            }
        }
        _ => {}
    }
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
                    || (key == "error" && value_reports_nonempty_error(value))
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

fn value_reports_nonempty_error(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        Value::Number(value) => value.as_f64() != Some(0.0),
        Value::Bool(true) => true,
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
    response_has_structured_failure(value)
        || (extract_agent_identifier(value).is_none() && response_has_textual_spawn_failure(value))
}

fn response_has_structured_failure(value: &Value) -> bool {
    match value {
        Value::Object(values) => {
            values.get("isError").and_then(Value::as_bool) == Some(true)
                || values.get("is_error").and_then(Value::as_bool) == Some(true)
                || values.get("error").is_some_and(|error| !error.is_null())
                || values.values().any(response_has_structured_failure)
        }
        Value::Array(values) => values.iter().any(response_has_structured_failure),
        _ => false,
    }
}

fn response_has_textual_spawn_failure(value: &Value) -> bool {
    match value {
        Value::Object(values) => ["content", "message", "output", "result", "text"]
            .iter()
            .filter_map(|key| values.get(*key))
            .any(response_has_textual_spawn_failure),
        Value::Array(values) => values.iter().any(response_has_textual_spawn_failure),
        Value::String(value) => {
            let normalized = value.trim().to_ascii_lowercase();
            [
                "collab spawn failed",
                "agent spawn failed",
                "spawn agent failed",
                "spawn_agent failed",
                "failed to spawn agent",
                "failed to spawn subagent",
            ]
            .iter()
            .any(|prefix| normalized.starts_with(prefix))
        }
        _ => false,
    }
}

fn extract_agent_identifier(value: &Value) -> Option<&str> {
    match value {
        Value::Object(values) => {
            for key in [
                "agent_id",
                "agentId",
                "agent_name",
                "agentName",
                "subagent_id",
                "subagentId",
            ] {
                if let Some(value) = values.get(key).and_then(Value::as_str) {
                    return Some(value);
                }
            }
            values.values().find_map(extract_agent_identifier)
        }
        Value::Array(values) => values.iter().find_map(extract_agent_identifier),
        _ => None,
    }
}

fn collect_terminal_task_ids(
    value: &Value,
    ledger: &SessionLedger,
    terminal_tasks: &mut BTreeSet<String>,
) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_terminal_task_ids(value, ledger, terminal_tasks);
            }
        }
        Value::Object(values) => {
            let terminal = values.iter().any(|(key, value)| {
                matches!(
                    normalized_identifier(key).as_str(),
                    "status" | "agentstatus" | "state"
                ) && value_reports_terminal(value)
            });
            if terminal {
                for key in [
                    "task_name",
                    "taskName",
                    "agent_name",
                    "agentName",
                    "agent_id",
                    "agentId",
                    "subagent_id",
                    "subagentId",
                ] {
                    if let Some(identifier) = values.get(key).and_then(Value::as_str)
                        && let Some(task_id) = ledger
                            .reservations
                            .keys()
                            .find(|task_id| identifier_mentions_task(identifier, task_id))
                    {
                        terminal_tasks.insert(task_id.clone());
                    }
                }
            }
            for value in values.values() {
                collect_terminal_task_ids(value, ledger, terminal_tasks);
            }
        }
        _ => {}
    }
}

fn value_reports_terminal(value: &Value) -> bool {
    match value {
        Value::String(value) => matches!(
            normalized_identifier(value).as_str(),
            "completed"
                | "errored"
                | "failed"
                | "shutdown"
                | "notfound"
                | "finalanswer"
                | "taskcomplete"
        ),
        Value::Object(values) => values.iter().any(|(key, value)| {
            matches!(
                normalized_identifier(key).as_str(),
                "completed"
                    | "errored"
                    | "failed"
                    | "shutdown"
                    | "notfound"
                    | "finalanswer"
                    | "taskcomplete"
            ) && !matches!(value, Value::Bool(false) | Value::Null)
        }),
        _ => false,
    }
}

fn spawn_task_id(tool_input: Option<&Value>) -> Option<&str> {
    let input = tool_input?.as_object()?;
    string_field(input, &["task_name", "taskName"])
}

fn string_field<'a>(values: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| values.get(*key).and_then(Value::as_str))
}

fn reservation_has_pending_acceptance(reservation: &Reservation) -> bool {
    reservation.acceptance.iter().any(acceptance_blocks_turn)
}

fn acceptance_blocks_turn(check: &AcceptanceEntry) -> bool {
    match check.status {
        AcceptanceStatus::Passed => false,
        AcceptanceStatus::Pending | AcceptanceStatus::Failed => true,
        AcceptanceStatus::Unverifiable => check.release_notice_delivered_at_ms.is_none(),
    }
}

fn identifier_mentions_task(identifier: &str, task_id: &str) -> bool {
    identifier == task_id
        || identifier
            .split(['/', ':'])
            .any(|component| component == task_id)
}

fn normalized_identifier(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn canonical_value_hash(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn hash_component(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn contract_input(task: &str, role: &str, contract: Value) -> Value {
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
            "checks": [{ "id": "tests", "cmd": "cargo test -p codey --lib" }]
        })
    }

    #[test]
    fn adaptive_contract_keeps_role_compatibility_without_dead_size_fields() {
        let input = contract_input("tiny", "codey_deep_research", research_contract("tiny"));
        assert!(prepare_contract(Some(&input)).is_ok());

        let quick = contract_input("quick", "codey_quick_scan", research_contract("quick"));
        assert!(
            prepare_contract(Some(&quick))
                .unwrap_err()
                .contains("why 与 agent_type 不兼容")
        );

        let unknown = contract_input(
            "unknown",
            "codey_quick_scan",
            json!({
                "id": "unknown",
                "why": "guess",
                "visual": false
            }),
        );
        assert!(
            prepare_contract(Some(&unknown))
                .unwrap_err()
                .contains("why 无效")
        );

        let huge_parallel = contract_input(
            "huge_parallel",
            "codey_quick_scan",
            json!({
                "id": "huge_parallel",
                "why": "parallel",
                "branch_calls": [65535, 65535],
                "visual": false,
                "read": [],
                "write": [],
                "checks": []
            }),
        );
        assert!(prepare_contract(Some(&huge_parallel)).is_ok());

        let user_requested = contract_input(
            "explicit",
            "codey_deep_research",
            json!({
                "id": "explicit",
                "why": "user_requested",
                "branch_calls": [10, 10, 10, 10],
                "visual": false,
                "read": [],
                "write": [],
                "checks": []
            }),
        );
        let prepared = prepare_contract(Some(&user_requested)).unwrap();
        assert_eq!(
            adaptive_limits(&prepared.contract, 1),
            (DEFAULT_POINT_LIMIT, DEFAULT_ATTEMPT_LIMIT)
        );

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
    fn encrypted_message_uses_conservative_workspace_contract() {
        let encrypted_input = json!({
            "task_name": "encrypted_worker",
            "agent_type": "codey_worker",
            "fork_turns": "none",
            "message": format!("gAAAAA{}", "A".repeat(160))
        });
        let prepared =
            prepare_contract_with_workspace(Some(&encrypted_input), Some("/repo")).unwrap();
        assert_eq!(prepared.contract.id, "encrypted_worker");
        assert_eq!(prepared.contract.reason, "encrypted_message");
        assert_eq!(prepared.workspace_root.as_deref(), Some("/repo"));
        assert_eq!(prepared.write_paths, ["/repo"]);
        assert!(prepared.read_paths.is_empty());
        assert!(prepared.contract.acceptance.is_empty());
        assert_eq!(
            adaptive_limits(&prepared.contract, 1),
            (DEFAULT_POINT_LIMIT, DEFAULT_ATTEMPT_LIMIT)
        );
        assert_eq!(
            adaptive_limits(&prepared.contract, 3),
            (9, DEFAULT_ATTEMPT_LIMIT)
        );
        assert_eq!(
            adaptive_limits(&prepared.contract, 5),
            (MAX_POINT_LIMIT, MAX_ATTEMPT_LIMIT)
        );

        let error = prepare_contract_with_workspace(Some(&encrypted_input), None).unwrap_err();
        assert!(error.contains("Hook 未提供绝对工作目录"));

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
    fn encrypted_spawn_budget_allows_the_bounded_maximum_attempts() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..MAX_ATTEMPT_LIMIT {
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
                    0,
                    u64::from(index),
                )
                .unwrap(),
                None
            );
        }

        let overflow = json!({
            "task_name": "opaque_overflow",
            "agent_type": "codey_quick_scan",
            "fork_turns": "none",
            "message": format!("gAAAAA{}", "A".repeat(160))
        });
        let denial = pre_spawn(
            temp.path(),
            "runtime-a",
            "session-a",
            Some(&overflow),
            0,
            100,
        )
        .unwrap()
        .unwrap();
        assert!(denial.contains("达到上限 6"));
    }

    #[test]
    fn failed_encrypted_spawns_do_not_inflate_the_adaptive_budget() {
        let temp = tempfile::tempdir().unwrap();
        for index in 0..DEFAULT_ATTEMPT_LIMIT {
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

        let retry = json!({
            "task_name": "failed_opaque_retry",
            "agent_type": "codey_quick_scan",
            "fork_turns": "none",
            "message": format!("gAAAAA{}", "A".repeat(160))
        });
        let denial = pre_spawn(temp.path(), "runtime-a", "session-a", Some(&retry), 0, 100)
            .unwrap()
            .unwrap();
        assert!(denial.contains("达到上限 4"));

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 110).unwrap().unwrap();
        assert_eq!(ledger.point_limit, DEFAULT_POINT_LIMIT);
        assert_eq!(ledger.attempt_limit, DEFAULT_ATTEMPT_LIMIT);
        assert_eq!(ledger.points_spent, 0);
        assert!(ledger.reservations.is_empty());
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
        assert_eq!(ledger.spawn_attempts, 1);
        assert_eq!(ledger.points_spent, 3);
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
    fn budget_denies_the_next_task_without_consuming_an_attempt() {
        let temp = tempfile::tempdir().unwrap();
        for (task, path, now_ms) in [
            ("worker_a", "backend/a.rs", 10),
            ("worker_b", "backend/b.rs", 20),
        ] {
            let input = contract_input(task, "codey_worker", worker_contract(task, path));
            assert_eq!(
                pre_spawn(
                    temp.path(),
                    "runtime-a",
                    "session-a",
                    Some(&input),
                    1,
                    now_ms
                )
                .unwrap(),
                None
            );
        }

        let third = contract_input(
            "worker_c",
            "codey_worker",
            worker_contract("worker_c", "backend/c.rs"),
        );
        let denial = pre_spawn(temp.path(), "runtime-a", "session-a", Some(&third), 2, 30)
            .unwrap()
            .unwrap();
        assert!(denial.contains("需要 3 点"));
        assert!(denial.contains("6/8 点"));

        let store = LedgerStore::open(temp.path(), "session-a").unwrap();
        let ledger = store.load("runtime-a", "session-a", 40).unwrap().unwrap();
        assert_eq!(ledger.spawn_attempts, 2);
        assert_eq!(ledger.points_spent, 6);
        assert!(!ledger.reservations.contains_key("worker_c"));
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
        assert_eq!(ledger.spawn_attempts, 2);
        assert_eq!(ledger.points_spent, 6);
        assert_eq!(ledger.reservations.len(), 2);
    }

    #[test]
    fn failed_spawn_refunds_points_but_keeps_the_attempt() {
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
        assert_eq!(ledger.spawn_attempts, 1);
        assert_eq!(ledger.points_spent, 0);
        assert!(ledger.reservations.is_empty());
    }

    #[test]
    fn textual_failed_spawn_releases_the_reservation() {
        for (index, response) in [
            Value::String("collab spawn failed: agent thread limit reached".to_string()),
            json!({
                "content": [{
                    "type": "text",
                    "text": "collab spawn failed: agent thread limit reached"
                }]
            }),
            json!({
                "agent_id": "agent-a",
                "isError": true,
                "error": "capacity"
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
            assert_eq!(ledger.spawn_attempts, 1);
            assert_eq!(ledger.points_spent, 0);
            assert!(ledger.reservations.is_empty());
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
        assert_eq!(ledger.spawn_attempts, 1);
        assert_eq!(ledger.points_spent, 3);
        assert_eq!(
            ledger.reservations["worker_a"].state,
            ReservationState::Running
        );
    }

    #[test]
    fn child_write_tool_must_stay_inside_declared_ownership() {
        let temp = tempfile::tempdir().unwrap();
        let input = contract_input(
            "worker_a",
            "codey_worker",
            worker_contract("worker_a", "backend/src"),
        );
        pre_spawn(temp.path(), "runtime-a", "session-a", Some(&input), 0, 10).unwrap();
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
        let denied_patch = json!({
            "patch": "*** Begin Patch\n*** Update File: README.md\n*** End Patch"
        });
        assert!(
            authorize_child_tool(
                temp.path(),
                "runtime-a",
                "session-a",
                "agent-a",
                "apply_patch",
                Some(&denied_patch),
                30,
            )
            .unwrap()
            .unwrap()
            .contains("ownership")
        );
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
        settle_turn(temp.path(), "runtime-a", "session-a", 70).unwrap();
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
        assert!(format!("{error:#}").contains("解析 Codey 子代理预算账本失败"));
        assert_eq!(std::fs::read(&ledger_path).unwrap(), b"{not-valid-json");
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
}
