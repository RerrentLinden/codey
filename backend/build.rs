use std::path::Path;
use std::process::Command;

fn main() {
    for path in [
        "../src",
        "../vite.overlay.config.ts",
        "../package.json",
        "../pnpm-lock.yaml",
        "icons/Codey.ico",
        "../scripts/build-overlay.mjs",
        "../public",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }

    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    let status = Command::new(npm)
        .args(["run", "vite:build"])
        .current_dir(Path::new(".."))
        .status()
        .expect("无法运行 npm 构建 Codey Web 配置页");
    assert!(status.success(), "Codey Web 配置页构建失败");

    #[cfg(windows)]
    embed_windows_icon();
}

#[cfg(windows)]
fn embed_windows_icon() {
    let mut resource = winres::WindowsResource::new();
    resource.set_icon("icons/Codey.ico").set_manifest(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
</assembly>"#,
    );
    resource.compile().expect("无法嵌入 Codey Windows 图标");
}
