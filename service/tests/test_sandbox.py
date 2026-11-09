"""Resource limits of the sandbox runner (uses coreutils/python, no CLI needed)."""
import dataclasses
import sys

from agentfx_service.sandbox import Sandbox


def test_wall_clock_kill(cfg):
    sb = Sandbox(cfg)
    d = sb.new_job_dir()
    r = sb.run(0, ["/bin/sleep", "30"], d, wall_s=1.0)
    assert r.killed == "timeout" and r.wall_s < 5


def test_kills_the_whole_process_group(cfg, tmp_path):
    sb = Sandbox(cfg)
    d = sb.new_job_dir()
    # a background child that would outlive its parent
    r = sb.run(0, ["/bin/sh", "-c", "sleep 31.7 & sleep 31.7"], d, wall_s=1.0)
    assert r.killed == "timeout"
    import subprocess
    out = subprocess.run(["pgrep", "-f", "sleep 31.7"], capture_output=True, text=True).stdout.split()
    assert not out


def test_file_size_limit(cfg):
    cfg = dataclasses.replace(cfg, file_mb=1)
    sb = Sandbox(cfg)
    d = sb.new_job_dir()
    r = sb.run(0, ["/bin/sh", "-c", "head -c 5000000 /dev/zero > big.bin"], d, wall_s=10)
    assert r.code != 0
    assert (d / "big.bin").stat().st_size <= 1024 * 1024


def test_job_disk_watchdog(cfg):
    cfg = dataclasses.replace(cfg, job_disk_mb=2, file_mb=64)
    sb = Sandbox(cfg)
    d = sb.new_job_dir()
    script = "for i in 1 2 3 4 5 6 7 8; do head -c 1000000 /dev/zero > f$i; done; sleep 20"
    r = sb.run(0, ["/bin/sh", "-c", script], d, wall_s=15)
    assert r.killed == "disk"


def test_memory_limit(cfg):
    cfg = dataclasses.replace(cfg, mem_mb=256)
    sb = Sandbox(cfg)
    d = sb.new_job_dir()
    r = sb.run(0, [sys.executable, "-c", "x = bytearray(600 * 1024 * 1024); print('allocated')"], d, wall_s=20)
    assert r.code != 0 and "allocated" not in r.stdout


def test_clean_environment(cfg, monkeypatch):
    monkeypatch.setenv("AFX_SIGNING_KEY", "secret-should-not-leak")
    monkeypatch.setenv("ANTHROPIC_API_KEY", "sk-should-not-leak")
    sb = Sandbox(cfg)
    d = sb.new_job_dir()
    r = sb.run(0, ["/usr/bin/env"], d, wall_s=5)
    assert "should-not-leak" not in r.stdout
    assert "AGENTFX_TTS=none" in r.stdout and f"HOME={d}" in r.stdout


def test_output_files_private(cfg):
    sb = Sandbox(cfg)
    d = sb.new_job_dir()
    sb.run(0, ["/bin/sh", "-c", "echo x > out.txt"], d, wall_s=5)
    assert (d / "out.txt").stat().st_mode & 0o077 == 0
    assert d.stat().st_mode & 0o077 == 0
