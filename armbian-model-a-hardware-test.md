# UOS TV Armbian Model A — Hardware Test Notes

## Test Context

- Branch: `armbian-base-model-a`
- Approach: Model A — modify existing proven Armbian image
- Target hardware: Amlogic S905X STB, B860H/HG680P family
- Image status: CI success, burned to SD card, boot tested on hardware
- Boot result: **Bootable** — device reaches Armbian login prompt

## Observed Boot Result

The device boots into:

```text
armbian login: root (automatic login)
```

It does **not** automatically open the UOS browser/UI.

## Systemd Status

Command:

```bash
systemctl --failed
```

Result:

```text
0 failed
```

Command:

```bash
systemctl status uos-shell.service --no-pager -l
systemctl status stardustd.service --no-pager -l
```

Result:

```text
uos-shell.service: active, enabled
stardustd.service: active, enabled
```

## Browser Runtime Check

Commands:

```bash
which startx
which Xorg
which openbox
which chromium
which chromium-browser
```

Results:

```text
which startx: no result
which Xorg: no result
which openbox: no result
which chromium: no result
which chromium-browser: no result
```

Manual launcher test:

```bash
/usr/bin/uos-shell-launcher
```

Result:

```text
No browser runtime found. Install chromium or cog.
```

## UOS Service List

Command:

```bash
systemctl list-units --type=service | grep -E 'uos|stardust|lumin|netm|wifi|audio|input'
```

Result:

```text
stardustd.service
uos-shell.service
```

Only `stardustd` and `uos-shell` are registered as systemd services.

## UOS Binary Check

Command:

```bash
ls -lah /usr/bin | grep -E 'stardust|lumin|netm|wifi|audio|input|uos'
```

Result includes:

```text
audiod
inputd
lumind
netmd
stardustd
uos-browser
uos-shell-launcher
wifid
```

## Current Findings

1. The SD image is **bootable**.
2. Armbian first-login `@issues` problem is no longer blocking boot.
3. `uos-shell.service` and `stardustd.service` are active and enabled.
4. Browser/runtime stack is missing from rootfs:
   - no `startx`
   - no `Xorg`
   - no `openbox`
   - no `chromium`
   - no `chromium-browser`
5. UOS binaries are present in `/usr/bin`.
6. Only two systemd services are currently installed/enabled:
   - `stardustd.service`
   - `uos-shell.service`
7. Other UOS daemon binaries exist but do not yet have generated/enabled systemd units in Model A image:
   - `audiod`
   - `inputd`
   - `lumind`
   - `netmd`
   - `wifid`

## Immediate Root Cause Candidate

The CI package install step likely failed or was skipped, but the workflow continued because `scripts/customize-armbian-model-a.sh` currently treats package installation failure as non-fatal:

```bash
apt-get install ... || {
  echo "WARN: package install failed or timed out; continuing with debug image" >&2
}
```

Because of this, the final image can still be produced without required browser/X11 packages.

## Required Fix Direction

1. Make browser/runtime package installation mandatory for browser-enabled image.
2. Fail CI if these commands are missing after customization:

```bash
command -v startx
command -v Xorg
command -v openbox
command -v chromium || command -v chromium-browser || command -v cog
```

3. Add a post-customization verification step in CI.
4. Optionally split image variants:
   - `debug-full`: must include browser/runtime stack
   - `boot-only`: allowed to omit browser, but must be clearly named

## Next Debug Command for CI

Inspect CI log around apt install:

```bash
gh run view 26999805152 --log | grep -iE "apt-get|chromium|xserver|openbox|package install failed|WARN"
```
