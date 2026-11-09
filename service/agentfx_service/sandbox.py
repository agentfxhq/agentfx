"""Run the CLI on untrusted input: throwaway job dir, own uid per worker slot, rlimits, wall
clock, job dir size watchdog, clean environment, no network (enforced outside this module by
an iptables owner rule for the sandbox uid range, see service/docker/entrypoint.sh).

The uid switch uses Popen(user=, group=, extra_groups=[]) (C code, no preexec_fn) and the
rlimits use the `prlimit` wrapper from util-linux, which sets them on itself and execs the
command, so nothing runs Python code between fork and exec.
"""
from __future__ import annotations

import logging
import os
import shutil
import signal
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

log = logging.getLogger("agentfx.sandbox")

PRLIMIT = shutil.which("prlimit") or "/usr/bin/prlimit"
OUT_CAP = 4 * 1024 * 1024  # bytes of stdout/stderr kept per process


class SandboxError(RuntimeError):
    def __init__(self, message, code="sandbox_error"):
        super().__init__(message)
        self.code = code


@dataclass
class Result:
    code: int
    stdout: str
    stderr: str
    wall_s: float
    killed: str | None = None  # "timeout" | "disk" | None


class Sandbox:
    def __init__(self, cfg):
        self.cfg = cfg
        self.uid_switch = False
        if cfg.sandbox_uid_base:
            if os.geteuid() == 0:
                self.uid_switch = True
            elif cfg.require_sandbox:
                raise SandboxError("AFX_SANDBOX_UID_BASE is set but the service is not root: cannot switch uids")
            else:
                log.warning("sandbox: not root, renders run as the service user (development only)")
        elif cfg.require_sandbox:
            raise SandboxError("AFX_REQUIRE_SANDBOX=1 needs AFX_SANDBOX_UID_BASE")
        else:
            log.warning("sandbox: AFX_SANDBOX_UID_BASE not set, renders run as the service user (development only)")
        if self.uid_switch:
            # service files (keys DB + WAL, signing key, assets, outputs) stay private; the data
            # dir is only traversable so that the sandbox uids reach their job dirs
            os.umask(0o077)
        for d in (cfg.jobs_dir, cfg.data_dir / "logs"):
            d.mkdir(parents=True, exist_ok=True)
        os.chmod(cfg.data_dir, 0o711)
        os.chmod(cfg.jobs_dir, 0o711)

    # ------------------------------------------------------------ job dirs

    def uid(self, slot: int) -> int | None:
        return self.cfg.sandbox_uid_base + slot if self.uid_switch else None

    def new_job_dir(self, prefix="job-") -> Path:
        d = Path(tempfile.mkdtemp(prefix=prefix, dir=self.cfg.jobs_dir))
        os.chmod(d, 0o700)
        return d

    def hand_over(self, job_dir: Path, slot: int):
        """Give the job dir and everything in it to the slot uid (before each run: files the
        service wrote in between belong to the service)."""
        uid = self.uid(slot)
        if uid is None:
            return
        os.lchown(job_dir, uid, uid)
        for root, dirs, files in os.walk(job_dir):
            for name in dirs + files:
                try:
                    os.lchown(os.path.join(root, name), uid, uid)
                except FileNotFoundError:
                    pass

    def remove(self, job_dir: Path):
        shutil.rmtree(job_dir, ignore_errors=True)

    # ------------------------------------------------------------ running

    def env(self, job_dir: Path, extra: dict | None = None) -> dict:
        threads = str(self.cfg.render_threads)
        e = {
            "PATH": "/usr/local/bin:/usr/bin:/bin",
            "HOME": str(job_dir),
            "TMPDIR": str(job_dir / ".tmp"),
            "XDG_CACHE_HOME": str(job_dir / ".cache"),
            "AGENTFX_CACHE": str(job_dir / ".cache"),
            "AGENTFX_HOME": self.cfg.agentfx_home,
            "AGENTFX_TTS": "none",
            "AGENTFX_GPU": "0",
            "AGENTFX_CALLER": "agentfx-service",
            "RAYON_NUM_THREADS": threads,
            "AGENTFX_OFFSCREEN_THREADS": threads,
            "LANG": "C.UTF-8",
        }
        renderer = os.path.join(os.path.dirname(self.cfg.agentfx_bin), "agentfx-render")
        if os.path.isfile(renderer):
            e["AGENTFX_RENDERER"] = renderer
        if extra:
            e.update(extra)
        return e

    def run(self, slot: int, argv: list, job_dir: Path, wall_s: float, env: dict | None = None,
            cpu_s: float | None = None) -> Result:
        """Run argv (argv[0] = executable) in job_dir as the slot uid with limits. Blocking."""
        cfg = self.cfg
        (job_dir / ".tmp").mkdir(exist_ok=True)
        (job_dir / ".cache").mkdir(exist_ok=True)
        self.hand_over(job_dir, slot)
        cpu = int(cpu_s or wall_s * max(1, cfg.render_threads) + 10)
        limits = [f"--as={cfg.mem_mb * 1024 * 1024}", f"--fsize={cfg.file_mb * 1024 * 1024}",
                  f"--cpu={cpu}", f"--nofile={cfg.nofile}", "--core=0", "--memlock=0"]
        if self.uid_switch:
            limits.append(f"--nproc={cfg.nproc}")
        cmd = [PRLIMIT, *limits, "--", *[str(a) for a in argv]]
        logs = cfg.data_dir / "logs"
        out_f = tempfile.TemporaryFile(dir=logs)
        err_f = tempfile.TemporaryFile(dir=logs)
        kw = {}
        uid = self.uid(slot)
        if uid is not None:
            kw.update(user=uid, group=uid, extra_groups=[])
        t0 = time.monotonic()
        try:
            p = subprocess.Popen(cmd, cwd=job_dir, env=self.env(job_dir, env), stdin=subprocess.DEVNULL,
                                 stdout=out_f, stderr=err_f, start_new_session=True, umask=0o077,
                                 close_fds=True, **kw)
        except OSError as e:
            out_f.close(), err_f.close()
            raise SandboxError(f"cannot start the renderer: {e}") from e
        killed = None
        last_du = 0.0
        while True:
            try:
                p.wait(timeout=0.5)
                break
            except subprocess.TimeoutExpired:
                pass
            now = time.monotonic()
            if now - t0 > wall_s:
                killed = "timeout"
            elif now - last_du > 2.0:
                last_du = now
                if dir_size(job_dir) > cfg.job_disk_mb * 1024 * 1024:
                    killed = "disk"
            if killed:
                kill_group(p)
                p.wait()
                break
        # the process group may have left children (ffmpeg) behind
        kill_group(p)
        res = Result(p.returncode, _tail(out_f), _tail(err_f), time.monotonic() - t0, killed)
        out_f.close(), err_f.close()
        return res


def kill_group(p):
    try:
        os.killpg(p.pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        pass


def _tail(f) -> str:
    f.seek(0, os.SEEK_END)
    n = f.tell()
    f.seek(max(0, n - OUT_CAP))
    return f.read().decode("utf-8", "replace")


def dir_size(d: Path) -> int:
    total = 0
    for root, _dirs, files in os.walk(d):
        for name in files:
            try:
                total += os.lstat(os.path.join(root, name)).st_size
            except OSError:
                pass
    return total
