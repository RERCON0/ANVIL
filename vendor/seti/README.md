# Seti icon inputs

`seti.woff` is byte-identical to the [VS Code Seti font at commit 8993bc4913b5ecd98a639457ac1dabf5e4c58787](https://github.com/microsoft/vscode/blob/8993bc4913b5ecd98a639457ac1dabf5e4c58787/extensions/theme-seti/icons/seti.woff).
The theme originates in [Seti UI](https://github.com/jesseweed/seti-ui), with the MIT notices in `seti-LICENSE.txt`.

`setiIconMap.ts` is the generated theme mapping inherited from the Helm fork,
not an unmodified upstream TypeScript file. Its original generator revision
was not retained; the exact reviewed input is pinned below. Do not infer an
upstream source revision from the generated file.

Git preserves this reviewed mapping byte for byte, including its CRLF line
endings, so fresh Linux and Windows checkouts match the same SHA-256 pin.

`scripts/gen_seti_icons.py` generates the committed font and Rust table from
these inputs. CI installs the hash-pinned fontTools wheel from
`scripts/requirements-icons.txt` and tests deterministic regeneration.

| Input | SHA-256 |
|---|---|
| seti.woff | b127762058f89b37b08d76185e3b0558f5c28859e39a3840a27cb28b9739d6e2 |
| setiIconMap.ts | 059dba2817a5d03e9eaddddb57b978c46e1244b3a2ca64ff8f205f3cb3bc9d05 |
| seti-LICENSE.txt | c67ddfeba3a0aaffe9e9f7165c155c06de5c8db609a92b3a1a29ee6ab316a0bb |
