# Security policy

SerialLoom installs a driver and runs some operations as administrator, so security reports matter to us.

## Reporting a vulnerability

Please **do not** open a public issue. Use GitHub's [private vulnerability reporting](https://github.com/RahulP5/SerialLoom/security/advisories/new) and include:

- what an attacker could do, and under which conditions
- steps to reproduce, or a proof of concept
- the SerialLoom and Windows versions you tested

You will get an answer within 7 days. We will tell you when a fix ships and credit you if you wish.

## Areas of interest

- The elevated helper (`serialloom.exe --serialloom-admin`) and its argument handling
- SerialLoom Helper, the on-demand service (`--serialloom-service`, `driver/native/helper.rs`): its pipe security, caller checks and the operations it allows ([docs/AUTOMATION.md](docs/AUTOMATION.md#serialloom-helper))
- The automation API (`automation.rs`, `winpipe.rs`): per-user pipe access, request parsing and limits
- Per-PC driver signing (`localsign.rs`): the catalog hash check and private-key deletion
- The driver (`drivers/serialloom-port`): request handling, buffer bounds, access control between the COM side and the control side
- The TCP and UDP endpoints. They have no authentication; bind them to trusted networks only. A UDP endpoint listens on this PC only (`127.0.0.1`) unless set otherwise.
- Traffic capture to file (`capture.rs`): where files are created, and the size and free-space limits

## Supported versions

Only the latest release gets security fixes.
