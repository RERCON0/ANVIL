# Memory and idle CPU measurements — 2026-10-08

These are local measurements of the ANVIL 0.1.1 release build at
`2d4f8585ce8b8a555848faf9d8ad06c56bec88d5`, not a fixed memory promise or a
comparison benchmark against another terminal. No model prompt was submitted.

## Results

All numbers are **MiB** (1,048,576 bytes). ANVIL and external CLI processes are
reported separately. The table uses median per-second readings; the peak column
is the largest sampled private working set, not an instrumented allocation peak.

| Scenario | ANVIL private working set | Sampled peak | Total working set | Private commit | Child processes, private working set |
|---|---:|---:|---:|---:|---:|
| One PowerShell pane, idle | 35.6 | 35.7 | 115.1 | 56.0 | 27.0 |
| One tab: Codex, OpenCode and one Git panel, idle | 135.1 | 135.5 | 221.9 | 158.6 | 219.5 |
| Four tabs, 20 panes, active Unicode/truecolor output | 1,340.3 | 1,342.8 | 1,418.1 | 1,442.4 | 397.0 |
| Same 20 panes after output stopped, history retained | 1,341.6 | 1,341.8 | 1,419.5 | 1,443.7 | 390.8 |
| Same panes after reducing scrollback to 1,000 rows | 260.7 | — | 338.6 | 354.2 | 392.2 |

The final row includes the configuration reload transition. Its sampled peak
still belongs to the old 25,000-row history, so a peak for the settled smaller
history is deliberately not reported. The last reading was 260.7 MiB private
working set and 354.5 MiB private commit.

The heavy scenario completed **2,160,000 long lines** from 18 writers; each writer
flushed 120,000 lines in 60.0 seconds and produced a completion receipt. Six
writers were visible, twelve ran in background tabs, and two other panes kept
real Codex/OpenCode sessions alive. Output contained changing truecolor escapes,
Cyrillic, CJK, block/Braille characters and combining accents. The default
25,000 scrollback rows per pane were overwritten repeatedly.

The application stayed running and did not reach the harness's 4 GiB private
commit ceiling. Retained history accounts for much of this workload's memory:
reducing its configured limit released roughly 1 GiB while all 20 panes remained
alive. This is evidence from a bounded stress run, not proof that every workload
is leak-free. An earlier uninstrumented run gave a similar 1,345.5 MiB sampled
peak, but only the receipt-verified run supplies the completed line count above.

## Conditions and counters

- Windows 11 Pro, build 10.0.26300; Intel Core i9-13900HX; approximately 32 GiB RAM.
- Intel UHD Graphics and NVIDIA RTX 4070 Laptop GPU installed. The test does not
  measure dedicated GPU memory or attribute a rendering adapter.
- Native x64 MSVC release, Rust 1.92.0, the tracked release optimization profile.
- 1920 × 1200 physical window, Consolas 15, default dark palette. Quotas and the
  Claude statusLine integration were disabled. The agent and heavy-output runs
  installed fallback fonts on demand; the plain-shell run did not.
- A 30-second warm-up, then 30 one-second idle readings. Heavy output was sampled
  for 65 seconds, followed by 20 seconds with retained history and 20 seconds
  after reducing the history limit. Completion receipts gate the latter phases.
- Real installed Codex/OpenCode CLIs at their interactive screens; synthetic
  terminal output supplies the load. No LLM generation, compilation workload or
  terminal input replay is implied by the line count.

The harness uses Windows
[PROCESS_MEMORY_COUNTERS_EX2](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex2)
through `GetProcessMemoryInfo`. Private working set counts resident pages private
to the process; total working set includes shared resident pages; private commit
counts committed private memory, including pages that need not be resident.
These counters require Windows 10/11 with the September 2023 cumulative update
or later. Windows performance counters were also checked during the heavy run.

Working-set numbers depend on fonts, terminal dimensions, history, driver and
system memory pressure. Do not substitute one counter for another or include
agent RAM in the `anvil.exe` result. The approximately 36 MiB plain-shell result
does not describe the 135 MiB agent workspace or the stress scenario.

Measured `anvil.exe` SHA-256:

```text
cca4f5a7f93c457bcad64d8e65d8d17f926dd97b19a26265285849c7fcd00776
```

## Reproduce

Install Python 3.13, Codex CLI and OpenCode. Use a release executable and a new
output directory outside the checkout:

```powershell
python -B scripts\bench_load.py --output D:\Bench\ANVIL-run-1
# Optional: --scenario one-shell | two-agents-git | heavy-output
```

The script creates isolated ANVIL configuration/session paths and restores the
normal CLI environment inside the test profiles. It records per-second counters
in `samples.json`, aggregates in `summary.json`, and writer receipts in the
scenario directory. A Windows Job Object owns the spawned test process and its
descendants; closing it stops only that test tree. Existing ANVIL windows are
untouched. The output phase has a 180-second deadline and the host private commit
has a 4 GiB ceiling. The heavy test takes about 135 seconds including warm-up;
all three scenarios take about four minutes.

Raw test configuration contains local installation/project paths. Keep that
scratch output local; the aggregate results above contain no credentials.

## Additional performance-audit idle samples

The benchmark now records ANVIL's kernel plus user CPU time through
`GetProcessTimes`. CPU below uses the difference between the first and last
sample divided by elapsed time, with a second column normalized to this
machine's 32 logical processors. It excludes child-process CPU.

| Plain PowerShell, 30-second warm-up + 60-second idle sample | One logical processor | Whole machine | Median private working set |
|---|---:|---:|---:|
| Before additional performance fixes, executable from `51de931` | 0.682% | 0.021% | 35.3 MiB |
| Development release candidate, first run | 0.784% | 0.025% | 42.0 MiB |
| Same candidate, repeat without concurrent build/test work | 1.101% | 0.034% | 40.1 MiB |

These samples **do not demonstrate an idle CPU improvement**. In particular,
they do not support the static audit's estimated 1–5% CPU saving. Foreground
window ownership, CPU frequency and graphics-driver activity were not pinned,
so the small absolute differences are not an isolated causal benchmark. The
candidate also retains additional bounded text caches and embeds raw icon
pixels; the measured working set increased in these runs. The original stress
results above remain measurements of their explicitly identified older binary,
not refreshed results for this candidate.

The changed scheduling separates one-second discovery from drawing: an
unchanged maintenance tick does not request a terminal frame. Blink, terminal
output, an open Git panel, pending saves and user input can still request frames.
Cursor blink is disabled in the default configuration, although terminal
applications can request it. No new claim about frame-time p95, startup savings
or Git polling cost is inferred from these idle samples.

Development candidate executable SHA-256:

```text
b3aca51f2a337387085df7814aaad86bc62a493d22bf78d5de1bee46768c135b
```

It was built with `cargo build --locked --release --bin anvil
--bin anvil-claude-status` using Rust 1.92.0 and the tracked release profile;
it is not the final signed-package benchmark. Repeat the idle sample with:

```powershell
python -B scripts\bench_load.py --scenario one-shell --idle-seconds 60 --output D:\Bench\ANVIL-idle-1
```

Finding-by-finding conclusions and rejected recommendations are in
[OPTIMIZATION-REVIEW](OPTIMIZATION-REVIEW.md).

## По-русски

Проверен не только сценарий с Codex, OpenCode и Git. Добавлены пустая оболочка и
нагрузка на четыре вкладки с 20 пейнами: 18 генераторов вывели 2,16 млн цветных
Unicode-строк. Память самого ANVIL: около 36 МиБ в пустой оболочке, 135 МиБ с
двумя агентами и Git, пик 1,31 ГиБ под этой нагрузкой. После сокращения истории
с 25 000 до 1 000 строк на пейн осталось около 261 МиБ. Память внешних CLI
приведена отдельно; постоянные «33 МБ» для всех сценариев не заявляются.
