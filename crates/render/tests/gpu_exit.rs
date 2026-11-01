//! A process that opened a GPU device leaves without the driver's exit-time teardown, which
//! intermittently ended finished renders with SIGSEGV (no message) on NVIDIA Vulkan: `agentfx
//! frame` / `sheet` with AGENTFX_GPU=1 reported "agentfx-render failed". Skipped without a GPU.
use std::process::Command;

const IR: &str = r#"{"version":1,"width":320,"height":180,"fps":30,"frames":1,"nodes":[{"name":"n","children":[],"origin":[160,90],"opacity":1.0,"content":{"type":"gen","kind":"noise","rect":[0,0,320,180],"params":{"flow":0.25},"palette":[[0.0,0.0,0.1,1.0],[1.0,0.1,0.7,1.0],[0.0,0.9,1.0,1.0]],"seed":0}}],"tracks":[],"assets":[],"fonts":[]}"#;

#[test]
fn gpu_process_exits_cleanly() {
    let dir = std::env::temp_dir().join(format!("agentfx-gpu-exit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let ir = dir.join("gen.ir.json");
    std::fs::write(&ir, IR).unwrap();
    let runs: usize = std::env::var("AGENTFX_GPU_EXIT_RUNS").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
    for i in 0..runs {
        let out = Command::new(env!("CARGO_BIN_EXE_agentfx-render")).arg("render").arg(&ir).args(["--no-encode", "--gpu=on", "--out"]).arg(dir.join("x.mp4")).output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        if i == 0 && !err.contains("GPU effects on") {
            eprintln!("no GPU: skipped ({err})");
            break;
        }
        assert!(out.status.success(), "run {i}: {} {err}", out.status);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
