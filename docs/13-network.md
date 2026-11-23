# 13 — Network Control (M11)

A per-app network view with priorities and limits, in the spirit of NetLimiter,
built only from what Windows provides. No kernel driver.

Items marked **(verify)** must be confirmed with a probe before being relied on.

## What NetLimiter does, and why Verity cannot copy it

Research (NetLimiter's documentation): NetLimiter is a kernel driver, a service
that feeds the driver the user's rules, and a UI. The driver intercepts
incoming and outgoing connections to block them or limit their speed. Setting
an app to Critical or High limits the traffic of every app with a lower
priority. It also has firewall rules, quotas and per-app statistics.

Verity's safety rules forbid installing or loading a kernel driver
(`09-safety.md` §2). That rule stays for good reasons: a driver needs
Microsoft attestation signing, a driver bug crashes the PC, and kernel
anti-cheat treats third-party network drivers with suspicion. Without a
driver, **no tool can slow down an app's downloads or reorder incoming
traffic.** Verity says so on the screen.

## What Windows offers without a driver

| Feature | Mechanism | Works for | Limits |
|---|---|---|---|
| Live per-app usage | ETW: Microsoft-Windows-Kernel-Network send/receive events (pid, size) **(verify)** payloads with a probe | Every app, TCP and UDP | Counts bytes; stores per-app totals only |
| Upload limit per app | Policy-based QoS: `MSFT_NetQosPolicySettingData` in `root\StandardCimv2` with `AppPathNameMatchCondition` and a throttle rate | Outbound traffic of that exe | Outbound only; units differ between docs **(verify)** with `Get-NetQosPolicy` |
| Priority tag per app | The same QoS policy with a DSCP value | Outbound traffic | Helps only where the network honours DSCP: on Wi-Fi, Windows maps DSCP to WMM access categories when the adapter driver supports QoS Map **(verify)**; most home routers ignore it on the internet side |
| Block an app | Windows Firewall rule through `INetFwPolicy2`, named `Verity: <app>` | In and out | Rule removal restores the prior state |
| Limit Windows Update downloads | Delivery Optimization background bandwidth policy **(verify)** value names | Windows Update and Store downloads | The only real download limit available |

Policy-based QoS created outside a domain applies DSCP only when
`HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\QoS` has `"Do not use NLA" = "1"`
(Microsoft KB 2733528, Cisco's Windows 10/11 guide); some reports say it still
fails on Windows 10 **(verify)**, and it may need a reboot **(verify)**.

## Priority levels

The user picks one level per app. What each level does is written next to it.

| Level | Does | Honest note shown |
|---|---|---|
| Critical | DSCP 46 (EF); never limited | "Marks this app's uploads as urgent. Helps on Wi-Fi with QoS support; most routers ignore it." |
| High | DSCP 34 (AF41); never limited | Same note |
| Normal | No change | |
| Low | DSCP 8 (CS1) and an upload limit (default 25% of measured upload) | "Slows this app's uploads so others go first. Downloads cannot be limited without a driver." |
| Blocked | Firewall rule blocking the app | "This app cannot use the internet until you unblock it." |

A custom upload limit (KB/s) can replace the Low default.

## Actions (new catalog entries)

| ID | Scope | Mechanism | Risk | Default |
|---|---|---|---|---|
| `net.app_upload_limit` | Persistent or session | QoS policy `Verity-<id>` with throttle, in the `localhost` store (persistent) or `ActiveStore` (session, gone on reboot) | Low | Off |
| `net.app_dscp` | Persistent or session | QoS policy `Verity-<id>` with DSCP | Low | Off |
| `net.app_block` | Persistent | Firewall rule `Verity: <app>` | Medium | Off |
| `net.do_background_limit` | Session | Delivery Optimization background limit during Session Mode | Low | Off |
| `net.qos_nla` | Persistent, machine | `"Do not use NLA"` value, needed for DSCP outside a domain; may need a reboot | Medium | Off; asked when a DSCP level is first chosen |

Every policy and rule Verity creates carries its prefix, so revert removes
exactly what Verity added and never touches the user's own policies or rules.
Revert after drift (the user edited Verity's policy) marks `superseded`.
All of these are machine-wide, so they need an elevated client
(`09-safety.md` §5).

## Measuring

Verity measures before it claims.

- **Per-app usage** is always measured from ETW while the Network screen is open
  or Session Mode is active.
- **Network test** (user-started, about 60 s): latency and jitter to the
  default gateway with `IcmpSendEcho2`, first idle, then while the user's own
  upload or download runs. It shows whether a Low or Critical setting changes
  local latency under load. Verity generates no internet traffic and contacts
  no server; the gateway is on the local network.
- A level without a measured effect is labelled "not proven on this network".

## Privacy

Stored: per-app byte totals per second. Never stored or shown: remote
addresses, ports, host names, packet contents (`09-safety.md` §7).

## IPC

| Method | Notes |
|---|---|
| `net.live` | per-app up and down rates and totals since the screen opened |
| `net.rules.list`, `net.rules.set`, `net.rules.delete` | elevated client for persistent rules |
| `net.test.start`, `net.test.result` | the gateway latency test |

## UI: the Network screen

- Live list: app, download rate, upload rate, total, level dropdown, upload
  limit field.
- A banner the first time: what Verity can and cannot control, in two lines.
- Test panel with the before/after latency chart.

## Amendments to `09-safety.md` (approved by the owner, 2026-11-23)

1. §3 "Makes network connections": allow ICMP echo to the default gateway only,
   during a network test the user starts. (The owner already decided on
   2026-11-16 that Verity need not be fully offline; this records the narrow
   case.)
2. §2 kernel-driver ban: **unchanged**. M11 is driver-free.
3. New HKLM writes: QoS policies, firewall rules, the Delivery Optimization
   value and `"Do not use NLA"`, all listed above.

## Dependencies

M3 (action framework and journal), M5 (ETW session code). M11 starts after M3
is merged.
