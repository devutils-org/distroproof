# Security policy

distroproof starts virtual machines and copies programs into them; envprobe
reads a machine's security settings. Security reports matter.

## Reporting a vulnerability

Please do not open a public issue. Use GitHub's
[private vulnerability reporting](https://github.com/devutils-org/distroproof/security/advisories/new)
with what you ran, what happened, and the version (`distroproof --version`).
You will get an answer within a week.

## Supported versions

Only the latest release gets fixes.

## Verifying a release

Each release archive carries a build provenance attestation:

```sh
gh attestation verify distroproof-v0.1.0-x86_64-unknown-linux-musl.tar.gz --repo devutils-org/distroproof
```
