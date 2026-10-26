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
- Per-PC driver signing (`localsign.rs`): the catalog hash check and private-key deletion
- The driver (`drivers/serialloom-port`): request handling, buffer bounds, access control between the COM side and the control side
- The TCP endpoints. They have no authentication; bind them to trusted networks only.

## Supported versions

Only the latest release gets security fixes.
