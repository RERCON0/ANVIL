"""Bounded Windows release-memory benchmark; real CLIs, no model prompt.

Run with --output pointing outside the checkout. Saves raw per-second counters.
APPDATA/LOCALAPPDATA are isolated for ANVIL; CLI profiles retain their usual paths.
The measured process and its children live in a kill-on-close Windows Job Object.
Private-working-set counters require Windows 10/11 updated since September 2023.
"""
import argparse
import ctypes as c
from ctypes import wintypes as w
import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]


def emit(marker, number):
    deadline = time.monotonic() + 180
    while not marker.exists():
        if time.monotonic() >= deadline:
            return
        time.sleep(.1)
    start = time.monotonic()
    sys.stdout.reconfigure(encoding="utf-8")
    for batch in range(1200):
        # 120,000 long colour/Unicode lines per pane, at 2,000 lines/second.
        text = "".join(f"\x1b[38;2;{batch % 256};{n % 256};140m{number}:{batch * 100 + n:06} "
                       + "Rust output \u0416\u4e2d\u2593\u28ff e\u0301 " * 6 + "\x1b[0m\r\n" for n in range(100))
        sys.stdout.write(text)
        sys.stdout.flush()
        delay = start + (batch + 1) * .05 - time.monotonic()
        if delay > 0:
            time.sleep(delay)
    completed = time.monotonic()
    marker.with_name(f"{marker.name}.{number}.done").write_text(
        json.dumps({"lines": 120000, "seconds": round(completed - start, 3)}), encoding="utf-8")
    # Stay alive through the two history measurements; exiting removes the pane.
    while time.monotonic() - completed < 90:
        time.sleep(.2)


class BasicLimits(c.Structure):
    _fields_ = [("user", c.c_int64), ("job_user", c.c_int64), ("flags", w.DWORD),
                ("min_ws", c.c_size_t), ("max_ws", c.c_size_t), ("active", w.DWORD),
                ("affinity", c.c_size_t), ("priority", w.DWORD), ("schedule", w.DWORD)]


class JobLimits(c.Structure):
    _fields_ = [("basic", BasicLimits), ("io", c.c_uint64 * 6),
                ("process_memory", c.c_size_t), ("job_memory", c.c_size_t),
                ("peak_process", c.c_size_t), ("peak_job", c.c_size_t)]


class Memory(c.Structure):
    _fields_ = [("cb", w.DWORD), ("faults", w.DWORD)] + [
        (name, c.c_size_t) for name in ["peak_ws", "ws", "peak_paged", "paged",
                                      "peak_nonpaged", "nonpaged", "commit", "peak_commit",
                                      "private_commit", "private_ws"]] + [("shared_commit", c.c_uint64)]


class Process(c.Structure):
    _fields_ = [("size", w.DWORD), ("usage", w.DWORD), ("pid", w.DWORD),
                ("heap", c.c_size_t), ("module", w.DWORD), ("threads", w.DWORD),
                ("parent", w.DWORD), ("priority", w.LONG), ("flags", w.DWORD), ("name", w.WCHAR * 260)]


def win_api():
    kernel = c.WinDLL("kernel32", use_last_error=True, winmode=0x800)
    psapi = c.WinDLL("psapi", use_last_error=True, winmode=0x800)
    ntdll = c.WinDLL("ntdll", use_last_error=True, winmode=0x800)
    for name, args, result in [
        ("CreateJobObjectW", [c.c_void_p, w.LPCWSTR], w.HANDLE),
        ("SetInformationJobObject", [w.HANDLE, c.c_int, c.c_void_p, w.DWORD], w.BOOL),
        ("AssignProcessToJobObject", [w.HANDLE, w.HANDLE], w.BOOL),
        ("OpenProcess", [w.DWORD, w.BOOL, w.DWORD], w.HANDLE),
        ("CloseHandle", [w.HANDLE], w.BOOL),
        ("CreateToolhelp32Snapshot", [w.DWORD, w.DWORD], w.HANDLE),
        ("Process32FirstW", [w.HANDLE, c.POINTER(Process)], w.BOOL),
        ("Process32NextW", [w.HANDLE, c.POINTER(Process)], w.BOOL),
    ]:
        function = getattr(kernel, name)
        function.argtypes, function.restype = args, result
    psapi.GetProcessMemoryInfo.argtypes = [w.HANDLE, c.POINTER(Memory), w.DWORD]
    psapi.GetProcessMemoryInfo.restype = w.BOOL
    ntdll.NtResumeProcess.argtypes, ntdll.NtResumeProcess.restype = [w.HANDLE], w.LONG
    return kernel, psapi, ntdll


def processes(kernel):
    handle = kernel.CreateToolhelp32Snapshot(2, 0)
    if handle == c.c_void_p(-1).value:
        raise c.WinError(c.get_last_error())
    try:
        entry = Process(size=c.sizeof(Process))
        result = []
        valid = kernel.Process32FirstW(handle, c.byref(entry))
        while valid:
            result.append((entry.pid, entry.parent, entry.name))
            valid = kernel.Process32NextW(handle, c.byref(entry))
        return result
    finally:
        kernel.CloseHandle(handle)


def counters(kernel, psapi, pid):
    handle = kernel.OpenProcess(0x410, False, pid)
    if not handle:
        return None
    try:
        data = Memory(cb=c.sizeof(Memory))
        if not psapi.GetProcessMemoryInfo(handle, c.byref(data), data.cb):
            raise c.WinError(c.get_last_error())
        return {key: getattr(data, key) for key in ["ws", "private_ws", "private_commit", "peak_ws"]}
    finally:
        kernel.CloseHandle(handle)


def sample(kernel, psapi, app):
    if app.poll() is not None:
        raise RuntimeError(f"ANVIL exited during sampling: {app.returncode}")
    entries = processes(kernel)
    owned = {app.pid}
    for _ in entries:
        found = {pid for pid, parent, _ in entries if parent in owned}
        if found <= owned:
            break
        owned |= found
    children = [(pid, name, counters(kernel, psapi, pid)) for pid, _, name in entries
                if pid in owned and pid != app.pid]
    return {"app": counters(kernel, psapi, app.pid), "children": children,
            "tree_private_ws": sum(data["private_ws"] for _, _, data in children if data is not None)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--exe", type=Path, default=ROOT / "target/release/anvil.exe")
    parser.add_argument("--project", type=Path, default=ROOT)
    parser.add_argument("--scenario", choices=["all", "one-shell", "two-agents-git", "heavy-output"], default="all")
    args = parser.parse_args()
    if os.name != "nt":
        parser.error("Windows is required")
    output = args.output.resolve()
    if output.is_relative_to(ROOT) or output.exists():
        parser.error("Use a new output directory outside the checkout")
    output.mkdir(parents=True)
    executable, project = args.exe.resolve(), args.project.resolve()
    native_codex = shutil.which("codex.exe")
    if native_codex:
        codex_command, codex_args = native_codex, []
    else:
        shim = shutil.which("codex.cmd")
        script = Path(shim).parent / "node_modules/@openai/codex/bin/codex.js" if shim else None
        codex_command, codex_args = shutil.which("node.exe"), [str(script)]
        if script is None or not script.is_file():
            parser.error("Install Codex CLI before benchmarking")
    opencode = shutil.which("opencode.exe")
    if not opencode or not codex_command:
        parser.error("Codex and OpenCode must be installed")
    kernel, psapi, ntdll = win_api()
    results = []
    completions = {}
    scenarios = ["one-shell", "two-agents-git", "heavy-output"] if args.scenario == "all" else [args.scenario]
    for scenario in scenarios:
        folder = output / scenario
        appdata, local = folder / "appdata", folder / "local"
        (appdata / "anvil").mkdir(parents=True)
        local.mkdir()
        marker = folder / "start-output"
        env = os.environ.copy()
        cli_env = {name: env[name] for name in ["APPDATA", "LOCALAPPDATA"] if name in env}
        env.update(APPDATA=str(appdata), LOCALAPPDATA=str(local))
        env.pop("NO_COLOR", None)
        env.pop("NODE_DISABLE_COLORS", None)
        profiles = [{"id": "bench-codex", "name": "Codex", "command": codex_command,
                     "args": codex_args, "env": cli_env},
                    {"id": "bench-opencode", "name": "OpenCode", "command": opencode,
                     "args": [], "env": cli_env},
                    {"id": "bench-shell", "name": "PowerShell",
                     "command": str(Path(os.environ["SystemRoot"]) / "System32/WindowsPowerShell/v1.0/powershell.exe"),
                     "args": ["-NoLogo", "-NoProfile"], "env": cli_env}]
        def pane(profile, git=False):
            return {"pane": {"profileId": profile, "cwd": str(project), "workspaceOpen": git,
                             "workspaceWidth": 380, "workspaceTab": "changes"}}
        def split(direction, children):
            return {"split": {"dir": direction, "children": [[1 / len(children), child] for child in children]}}
        tabs = [{"layout": split("Row", [pane("bench-codex", True), pane("bench-opencode")]), "focused": 0}]
        if scenario == "one-shell":
            tabs = [{"layout": pane("bench-shell"), "focused": 0}]
        if scenario == "heavy-output":
            for tab in range(3):
                rows = []
                for row in range(3):
                    items = []
                    for col in range(2):
                        number = tab * 6 + row * 2 + col
                        profile = f"output-{number}"
                        profiles.append({"id": profile, "name": profile, "command": sys.executable,
                                         "args": ["-I", "-u", str(Path(__file__).resolve()), "--emit",
                                                  str(marker), str(number)], "env": cli_env})
                        items.append(pane(profile))
                    rows.append(split("Row", items))
                tabs.append({"layout": split("Column", rows), "focused": 0})
        config = {"profiles": profiles, "font": {"family": "Consolas", "size": 15},
                  "terminal": {"scrollback": 25000}, "restoreSession": True, "restoreAgents": False,
                  "quota": {"enabled": False}, "claudeStatus": {"enabled": False}}
        config_path = appdata / "anvil/config.json"
        config_path.write_text(json.dumps(config), encoding="utf-8")
        session = {"window": {"x": 0, "y": 0, "width": 1920, "height": 1200, "maximized": False},
                   "activeTab": 1 if scenario == "heavy-output" else 0, "tabs": tabs}
        (appdata / "anvil/session.json").write_text(json.dumps(session), encoding="utf-8")
        job = kernel.CreateJobObjectW(None, None)
        if not job:
            raise c.WinError(c.get_last_error())
        limits = JobLimits()
        limits.basic.flags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        if not kernel.SetInformationJobObject(job, 9, c.byref(limits), c.sizeof(limits)):
            kernel.CloseHandle(job)
            raise c.WinError(c.get_last_error())
        startup = subprocess.STARTUPINFO()
        startup.dwFlags = subprocess.STARTF_USESHOWWINDOW
        startup.wShowWindow = 0
        app = None
        try:
            # CREATE_SUSPENDED: job owns children before any code runs.
            app = subprocess.Popen([str(executable)], cwd=project, env=env, startupinfo=startup,
                                   creationflags=4)
            if not kernel.AssignProcessToJobObject(job, int(app._handle)):
                app.kill()
                raise c.WinError(c.get_last_error())
            if ntdll.NtResumeProcess(int(app._handle)) < 0:
                raise RuntimeError("Cannot resume benchmark process")
            print(f"{scenario}: warming up 30 s, PID {app.pid}", flush=True)
            time.sleep(30)
            phases = [("idle", 30)] if scenario != "heavy-output" else [
                ("output", 65), ("retained-history", 20), ("history-reduced-to-1000", 20)]
            for phase, seconds in phases:
                if phase == "output":
                    marker.touch()
                elif phase == "history-reduced-to-1000":
                    config["terminal"]["scrollback"] = 1000
                    temporary = config_path.with_suffix(".tmp")
                    temporary.write_text(json.dumps(config), encoding="utf-8")
                    temporary.replace(config_path)
                print(f"{scenario}: {phase} ({seconds} s)", flush=True)
                start = time.monotonic()
                while True:
                    elapsed = time.monotonic() - start
                    done = sorted(folder.glob("start-output.*.done")) if phase == "output" else []
                    if elapsed >= seconds and (phase != "output" or len(done) == 18):
                        if phase == "output":
                            receipts = [json.loads(path.read_text(encoding="utf-8")) for path in done]
                            completions[scenario] = {"writers": len(receipts),
                                "lines": sum(receipt["lines"] for receipt in receipts),
                                "slowest_writer_seconds": max(receipt["seconds"] for receipt in receipts)}
                            print(f"Completed output: {completions[scenario]}", flush=True)
                        break
                    if phase == "output" and elapsed >= 180:
                        raise RuntimeError(f"Output deadline reached: {len(done)}/18 writers completed")
                    data = sample(kernel, psapi, app)
                    if data["app"]["private_commit"] > 4 * 1024**3:
                        raise RuntimeError("Benchmark memory ceiling reached (4 GiB ANVIL commit)")
                    data.update(scenario=scenario, phase=phase, second=round(time.monotonic() - start, 2))
                    results.append(data)
                    (output / "samples.json").write_text(json.dumps(results, indent=2), encoding="utf-8")
                    time.sleep(1)
        finally:
            kernel.CloseHandle(job)
            if app is not None:
                app.wait(timeout=15)
        print(f"{scenario}: owned process tree stopped", flush=True)
    summary = {"binary_sha256": hashlib.sha256(executable.read_bytes()).hexdigest(),
               "completed_output": completions, "scenarios": []}
    for scenario, phase in sorted({(s["scenario"], s["phase"]) for s in results}):
        group = [s for s in results if (s["scenario"], s["phase"]) == (scenario, phase)]
        row = {"scenario": scenario, "phase": phase, "samples": len(group)}
        for key in ["private_ws", "ws", "private_commit"]:
            values = [s["app"][key] / 1024**2 for s in group]
            row[key] = {"median_mib": round(statistics.median(values), 1), "peak_mib": round(max(values), 1),
                        "last_mib": round(values[-1], 1)}
        row["children_private_ws_median_mib"] = round(statistics.median(s["tree_private_ws"] for s in group) / 1024**2, 1)
        summary["scenarios"].append(row)
    (output / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    print(json.dumps(summary, indent=2), flush=True)


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--emit":
        emit(Path(sys.argv[2]), int(sys.argv[3]))
    else:
        main()
