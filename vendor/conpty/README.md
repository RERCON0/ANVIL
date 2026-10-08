# ConPTY from the Windows Terminal project

`x64/conpty.dll` and `x64/OpenConsole.exe` are ConPTY **1.23.251008001** (the
same build `node-pty` 1.2.0-beta.8 ships in
`third_party/conpty/1.23.251008001/win10-x64`). Source:
<https://github.com/microsoft/terminal>, MIT licence (see `LICENSE`).

Why ANVIL ships its own ConPTY: the one built into Windows re-encodes terminal
input for applications that requested win32-input-mode (`CSI ? 9001 h`) as one
key event per character, so arrow keys and terminal replies reach them as an
Escape key followed by plain text. It also drops mouse-mode resets
(`CSI ? 1003 l` and friends) written by an application on exit, which leaves
the terminal reporting mouse motion into the shell. This ConPTY passes VT
through unchanged.

`alacritty_terminal` loads it with `LoadLibraryW("conpty.dll")`, which looks in
the executable's directory first. `OpenConsole.exe` must sit next to
`conpty.dll`.

The files were independently re-derived from
`https://registry.npmjs.org/node-pty/-/node-pty-1.2.0-beta.8.tgz` on 2026-10-08.
Its SHA-256 is
`f4832644b8fe9a8b53d4e3114ff13e56b0d8c7311d66eeda05f5fc91898f7f12`;
both members are byte-identical to the vendored files. `publisher.json` records
this source and the reviewed Microsoft signer. Windows CI and packaging run
`scripts/check_conpty_signatures.ps1`, which requires a **valid cryptographic
Authenticode signature** and the pinned Microsoft certificate, in addition to
the per-file SHA-256 check. Reading a certificate subject alone is insufficient.
