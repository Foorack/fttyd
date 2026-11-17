# Security

## Reporting a vulnerability

Use GitHub's private reporting: **Security → Advisories → Report a vulnerability** on
[this repository](https://github.com/Foorack/fttyd/security/advisories/new). Please do not open a
public issue for anything exploitable.

Expect an acknowledgement within a few days. If the report is valid I will fix it and credit you in
the advisory unless you prefer otherwise.

## What counts as a vulnerability here

`fttyd`'s own code - for example a request that escapes `--file-root`, a path that writes outside the
target directory, a CSRF bypass on a mutating endpoint, an authentication bypass in `-c`/`-H`/`-O`, a
crash reachable from the network, or a memory-safety problem.

Problems in a dependency are worth reporting too, though they are usually better filed upstream; tell
me and I will bump or patch as appropriate.

## What is not a vulnerability

These are documented behaviour, not bugs:

- **The terminal is a shell.** Whoever gets past your authentication gets a shell with the server's
  privileges. That is the point of a web terminal.
- **The file API has the server's reach.** By default it sees what the process sees, because the
  terminal beside it already can. `--file-root` restricts the file API; it is not a jail for the
  shell, and it does not sandbox anything.
- **The file API cannot escalate.** It runs as the server's user and cannot `sudo`, so root-owned
  paths are listed as locked with the real error rather than prompting.
- **`/token` echoes the credential.** It is ttyd's behaviour and the value is only a CSRF token; the
  real credential is the HTTP Basic header or whatever your proxy checks.
- **There is no built-in user management.** Authentication is `-c` (HTTP Basic) or `-H` (trust a
  header set by an authenticating proxy). Anything beyond that is your proxy's job.

## The model, in one paragraph

Read-only is the default: without `-W`, clients cannot type into the terminal and every mutating file
request is refused. With `-W`, every non-GET request must carry a header whose value comes from
`/token` - a cross-origin page cannot set it, because no CORS policy is granted to approve the
preflight. The file API runs with the server's privileges, TLS is rustls (no TLS 1.0/1.1 at all) and
`-A` enforces client certificates. Behind `-H`, every mutation is logged with the authenticated
actor as reported by your proxy.
