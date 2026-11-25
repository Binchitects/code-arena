# Code Arena

Farzan Arena's coding agent, rewritten in Rust: one program for the terminal and an IDE, for
Linux, macOS and Windows, working with every Farzan Arena server from 8.0 on.

**Status:** early. The plan, its milestones and the parity checklist are in
[docs/plan.md](docs/plan.md); what each version brought is in [CHANGELOG.md](CHANGELOG.md).

## Using it

```bash
code-arena login --url https://DOMAIN     # asks for your API key (Your account → API key)
code-arena models                         # the models your key may use
code-arena doctor                         # what this Arena has, feature by feature
code-arena -p "list the TODOs in src/"    # answer once, and exit
```

## Building it

Everything builds inside the build image; nothing needs installing on the host but Docker.

```bash
docker build -t code-arena-builder docker/builder
scripts/in-builder.sh make ci     # format check, clippy, tests
scripts/in-builder.sh make dist   # a static Linux binary in dist/
```

Behind a proxy, set `HTTPS_PROXY` before `scripts/in-builder.sh` so crates.io can be reached.

## Licence

MIT. See [LICENSE](LICENSE).
