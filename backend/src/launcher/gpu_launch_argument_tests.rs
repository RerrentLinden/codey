use super::*;

#[test]
fn gpu_launch_arguments_are_mutually_exclusive_and_platform_gated() {
    assert!(gpu_launch_arguments(GpuLaunchMode::Off, true).is_empty());
    assert_eq!(
        gpu_launch_arguments(GpuLaunchMode::DisableGpu, true),
        vec![DISABLE_GPU_ARGUMENT.to_string()]
    );
    assert_eq!(
        gpu_launch_arguments(GpuLaunchMode::DisableGpuRasterization, true),
        vec![DISABLE_GPU_RASTERIZATION_ARGUMENT.to_string()]
    );
    assert!(gpu_launch_arguments(GpuLaunchMode::DisableGpu, false).is_empty());
    assert!(gpu_launch_arguments(GpuLaunchMode::DisableGpuRasterization, false).is_empty());
}

#[test]
fn runtime_arguments_set_chinese_before_the_renderer_starts() {
    assert_eq!(
        codex_runtime_arguments(GpuLaunchMode::Off, true, false),
        vec![DEFAULT_CHINESE_LOCALE_ARGUMENT.to_string()]
    );
    assert_eq!(
        codex_runtime_arguments(GpuLaunchMode::DisableGpu, true, false),
        vec![
            DEFAULT_CHINESE_LOCALE_ARGUMENT.to_string(),
            DISABLE_GPU_ARGUMENT.to_string(),
        ]
    );
}

#[test]
fn windows_runtime_arguments_disable_background_ecoqos() {
    assert_eq!(
        codex_runtime_arguments(GpuLaunchMode::Off, true, true),
        vec![
            DEFAULT_CHINESE_LOCALE_ARGUMENT.to_string(),
            DISABLE_BACKGROUND_ECOQOS_ARGUMENT.to_string(),
        ]
    );
}
