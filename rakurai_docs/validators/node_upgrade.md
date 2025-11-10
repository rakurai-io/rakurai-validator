# Rakurai Node Upgrade — Guide

Fast upgrade steps for operators already running a Rakurai validator.

**Audience:** Existing Rakurai validator operators upgrading to a new release.

---

## 1. Upgrade steps

If you are already running Rakurai and only want to upgrade your node, follow these steps for this release.

**Note:** If you are new to running Rakurai, follow the complete guide starting at [Setup and build](./setup_and_build.md#2-download-and-build-rakurai-solana).

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

---

## 2. Share diagnostics log after first leader turn

Please share the output of this command with the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official) after your first leader turn on a Rakurai validator:

```bash
date; for p in "block time" "Banking packet delay" "rakurai_status"; do line=$(grep "$p" <LOG_FILE> | tail -n1); echo "$p: ${line:-not found}"; done
```

Replace `<LOG_FILE>` with your actual log file name.
