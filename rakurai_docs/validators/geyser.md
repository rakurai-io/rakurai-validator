# Rakurai Geyser — Guide

> **Last updated:** `v4.3.0-rakurai.0` · Yellowstone gRPC `v16.0.0+solana.4.3.0`

**If you run a Geyser plugin, read this before upgrading to Rakurai.** A plugin built against stock Agave will not work on a Rakurai node, and it may crash the validator rather than fail cleanly.

The fix is to point your Geyser / Yellowstone crates at the Rakurai repository (and the matching Solana SDK patches), then clean-rebuild — a dependency change, not a code change. It has to be redone on every Rakurai release, because the branch / revision must match the validator version.

**Audience:** Validator operators running third-party Geyser plugins (including Yellowstone gRPC) with Rakurai.

---

## 1. Overview

The struct layout and padding of the **Rakurai Validator** are slightly different from the standard **Agave/Solana validator**. Because of this, you cannot directly run any Geyser with the Rakurai validator and vice versa. To run any Geyser with Rakurai Validator, you must use the Geyser-related crates from the Rakurai repository and build your Geyser with them.

A sample Geyser plugin binary is provided in this repository: **[spark-geyser](../../spark-geyser/README.md)**.

> [!CAUTION]
> **Never run an unpatched Geyser**
>
> Running a standard Geyser without applying the Rakurai-compatible patches **may crash your node or cause undefined behavior**. Always build against the Geyser crates from the Rakurai repository.

---

## 2. Required patch (`v4.3.0-rakurai.0` / Yellowstone `v16.0.0+solana.4.3.0`)

Apply the Rakurai Cargo.toml patch to your Geyser / Yellowstone gRPC workspace root:

**Patch file:** [`patches/yellowstone-v16.0.0-solana.4.3.0-rakurai.0.patch`](./patches/yellowstone-v16.0.0-solana.4.3.0-rakurai.0.patch)

```bash
# From the Yellowstone gRPC (or Geyser plugin) repo root
git apply /path/to/rakurai_docs/validators/patches/yellowstone-v16.0.0-solana.4.3.0-rakurai.0.patch
```

What the patch does:

1. Points Agave monorepo crates (`agave-geyser-plugin-interface`, `solana-account-decoder`, `solana-entry`, `solana-storage-proto`, `solana-transaction-context`, `solana-transaction-status`) at `rakurai-validator` branch `v4.3.0-rakurai.0`.
2. Pins `solana-message = "=4.5.0"` and `solana-transaction = "=4.2.0"`.
3. Adds `[patch.crates-io]` for `solana-message` / `solana-packet` / `solana-transaction` from `rakurai-io/solana-sdk` at rev `54da378…`.
4. Sets `[workspace.lints.rust] deprecated = "allow"`.

> [!CAUTION]
> **The branch / revision must match your validator version**
>
> Every Rakurai git `branch` in the patch must match the validator release you run (`v4.3.0-rakurai.0` for this guide). The `solana-sdk` `rev` must stay in sync with that release. A mismatch causes a struct/ABI mismatch that **may crash the validator**.

---

## 3. Build steps

After applying the patch:

```bash
cargo clean
cargo build --release
```

`cargo clean` is required, not optional: the patch changes which source the Geyser interface crates resolve to, and a stale `target/` directory can silently link the previous Agave-built objects.

Then run your Geyser plugin as usual.

---

## 4. Verify the build

Confirm the patch actually took effect before loading the plugin on a validator:

```bash
# Every agave-geyser-plugin-interface entry should resolve to the Rakurai git source,
# not to a registry (crates.io) source.
cargo tree -i agave-geyser-plugin-interface
```

If any entry still shows `registry+https://github.com/rust-lang/crates.io-index`, the patch did not apply — check that workspace deps and `[patch.crates-io]` are in the **top-level** `Cargo.toml` of your workspace rather than in a member crate.

---

## 5. Upgrading

Each time you upgrade the Rakurai validator:

1. Apply the matching release patch under (or update every `branch` / `solana-sdk` `rev` in `Cargo.toml` to the new release).
2. `cargo clean && cargo build --release`.
3. Replace the plugin `.so` and restart the validator with the plugin loaded.

Rebuild the Geyser plugin **before** restarting the validator on the new release, so the two never run against mismatched struct layouts.

---

## 6. Reference implementation

For a working plugin and its configuration, see [spark-geyser](../../spark-geyser/README.md) — a prebuilt, lightweight ZeroMQ forwarding plugin maintained alongside each Rakurai release.
