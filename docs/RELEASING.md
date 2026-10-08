# Signing and verifying ANVIL releases

ANVIL uses its own Ed25519 publisher key. A signed manifest lists every payload
file, its size and SHA-256, executable PE properties, source commit/tree, source
input digest, Rust/Cargo versions and the lockfile digest. The signing key stays
outside the repository and is never available to pull requests or CI.

## Verify a downloaded release

Use Python 3.13 and OpenSSL 3 (included with Git for Windows). Obtain the
verification scripts and public key from a trusted ANVIL checkout, independently
of the ZIP you are checking. From that checkout:

```powershell
python -B scripts\release.py verify C:\Downloads\anvil-windows-x64.zip
```

The verifier checks the signature before interpreting the manifest. It rejects
missing, extra or duplicate ZIP members, encrypted entries, non-regular files,
unsupported compression, oversized payloads, changed hashes and incorrect PE
architecture/subsystem or ASLR/DEP flags. It neither extracts nor runs the files.
The bundled key must match the independently trusted key.

Trusted key: [release/public-key.pem](../release/public-key.pem).
SHA-256 of its Ed25519 SubjectPublicKeyInfo DER:

```text
0b983da0a5070ae2c6489e2b4d4f4a7c7a7769a4cd2a32d810744a2d32bb39a2
```

> [!IMPORTANT]
> This is a package signature, not Windows Authenticode. It establishes that the
> payload was signed with the trusted ANVIL key; it does not grant SmartScreen
> reputation or prove that the program has no bugs. Windows may show a warning.
> If you trust the download source, use **More info → Run anyway**.

## Build and sign

The release build requires a clean committed checkout, native Windows x64
MSVC tools, the pinned Rust toolchain, Python 3.13 and OpenSSL 3. Compiler, target
and profile environment overrides are rejected. A fresh temporary target tree
prevents a stale executable from becoming a release artifact.

```powershell
python -B scripts\release.py build --private-key "$env:LOCALAPPDATA\ANVIL\release-signing\anvil-private.pem"
python -B scripts\release.py verify dist\anvil-windows-x64.zip
```

The output is `dist/anvil-windows-x64.zip`. The build checks the unsigned
candidate against its source revision, adds the signed inventory, then verifies
the finished archive before replacing the output file atomically. Source changes
during compilation reject the build. Do not publish the unsigned intermediate ZIP.

For a new publisher key, use `release.py keygen --private-key <outside-repo-path>`.
The command refuses to overwrite existing keys. Windows directory permissions
are restricted to the current account. Back up the private key securely; key
rotation requires announcing and independently distributing a new fingerprint.

## Release checklist

1. Complete formatting, Clippy, Rust/Python tests, dependency and secret scans.
2. Commit and push the reviewed sources; confirm CI and Security on that commit.
3. Build and verify the signed ZIP from that exact clean commit.
4. Tag the same commit. Upload the signed ZIP and an archive SHA-256, then publish the release notes.

CI only builds unsigned, clearly labelled test candidates; it has read-only
repository permissions and no publisher key. The detached signature is tested
with temporary unrelated keys, including modified payloads and wrong-key cases.

## Русский

Подпись Ed25519 покрывает файлы пакета, сведения об исходниках и сборке.
Проверяйте ZIP скриптом и ключом из независимо доверенного репозитория:

```powershell
python -B scripts\release.py verify C:\Downloads\anvil-windows-x64.zip
```

Нужны Python 3.13 и OpenSSL 3. Проверка ничего не распаковывает и не запускает.
Ключ из самого ZIP не считается источником доверия. Отпечаток указан выше.
Это не Authenticode: SmartScreen может предупредить. При доверии источнику:
**Подробнее → Выполнить в любом случае**.

Приватный ключ хранится вне Git и CI. Подписанный ZIP собирается из чистого
коммита в свежем каталоге; изменение исходников, лишние файлы пакета, неправильные
хеши или подпись завершают проверку ошибкой. CI-артефакты остаются неподписанными.
