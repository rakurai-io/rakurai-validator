# Rakurai Node Upgrade — Guide

**Upgrading is short, but a version mismatch takes the node down rather than degrading quietly.** The client and the scheduler library are released together and must match. Running a new client against an old scheduler binary, or losing `LD_LIBRARY_PATH` or XDP capabilities during the rebuild, will stop the node.

This page is the fast path for operators already on Rakurai: pull the release, rebuild, refresh the scheduler binary, and restart. First-time setup is a different guide.

**Audience:** Existing Rakurai validator operators upgrading to a new release.

---

## 1. Upgrade steps

If you are already running Rakurai and only want to upgrade your node, follow these steps for this release.

> [!NOTE]
> **First-time setup is a different guide**
>
> This page assumes Rakurai already runs on the node. If you are setting up for the first time, follow [Setup and build](./setup_and_build.md#2-download-and-build-rakurai-solana) instead — it also covers the activation account and CLI arguments, which an upgrade does not touch.

1. Check out the latest release:

    ```bash
    git checkout <RELEASE_TAG>
    ```

2. Update your submodules in this release:

    ```bash
    git submodule update --init
    ```

3. Download the [scheduler binary](./setup_and_build.md#233-download-the-scheduler-binary), authenticating it with your identity key.
4. Replace your old binary with the new one in the correct paths.
5. [Build and run](./setup_and_build.md#3-build-the-client) your validator, incorporating the scheduler binary you just downloaded.

> [!CAUTION]
> **The scheduler version must match the release tag**
>
> The scheduler library and the client are built against the same struct layout. Running a scheduler binary from a different release than the checked-out `<RELEASE_TAG>` can crash the validator through an ABI mismatch. Download the scheduler for the exact release you checked out in step 1.

> [!WARNING]
> **Two things a rebuild resets**
>
> - **XDP capabilities** are attached to the binary and are lost on rebuild — re-run [`setcap`](./setup_and_build.md#4-grant-capabilities-for-xdp-linux-only).
> - **`LD_LIBRARY_PATH`** must still point at the new `librak*.so` in your launch script or systemd unit.

Before restarting, confirm the binary you downloaded is genuine with [Binary attestation](./binary_attestation.md).

---

## 2. Share diagnostics log after first leader turn

Please share the output of this command with the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official) after your first leader turn on a Rakurai validator:

```bash
date; for p in "block time" "Banking packet delay" "rakurai_status"; do line=$(grep "$p" <LOG_FILE> | tail -n1); echo "$p: ${line:-not found}"; done
```

Replace `<LOG_FILE>` with your actual log file name.
