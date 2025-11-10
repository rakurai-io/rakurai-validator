# Verify Rakurai Scheduler Binary

The Rakurai scheduler binary comes with [GitHub artifact attestation](https://docs.github.com/en/actions/concepts/security/artifact-attestations), providing cryptographic proof of its build provenance and integrity. This document describes how to verify the binary's attestations (provenance and SBOM) and checksum to ensure authenticity and integrity.

## Table of Contents

- [GitHub Artifact Attestations](#github-artifact-attestations)
- [Supply-chain Levels for Software Artifacts (SLSA) Compliance](#supply-chain-levels-for-software-artifacts-slsa-compliance)
- [Verify Rakurai scheduler binary attestation with the GitHub CLI](#verify-rakurai-scheduler-binary-attestation-with-the-github-cli)
- [Verify SBOM attestation with the GitHub CLI](#verify-sbom-attestation-with-the-github-cli)
- [Verify Rakurai scheduler binary checksum](#verify-rakurai-scheduler-binary-checksum)
- [Scheduler I/O Isolation Test](#scheduler-i/o-isolation-test)

## GitHub Artifact Attestations

Artifact attestations enable the creation of unfalsifiable provenance and integrity guarantees for software. In turn, software consumers can verify where and how the software was built. GitHub's artifact attestations create cryptographically signed claims that establish build provenance and include the following information:

- A link to the workflow associated with the artifact
- The repository, organization, environment, commit SHA, and triggering event for the artifact

For more information, see [Artifact attestations](https://docs.github.com/en/actions/concepts/security/artifact-attestations).

## Supply-chain Levels for Software Artifacts (SLSA) Compliance

GitHub artifact attestations are SLSA compliant. The SLSA framework is an industry standard used to evaluate supply chain security. This gives you confidence that binary hasn’t been tampered with after the build and can be securely traced back to its source.
For more information, see [SLSA](https://slsa.dev/).

## Verify Rakurai scheduler binary attestation with the GitHub CLI

Rakurai generates an attestation for every release. The following steps show how to verify it using the GitHub CLI.

### Prerequisites

- [GitHub CLI](https://github.com/cli/cli#installation) installed

### Verification

To verify the Rakurai scheduler binary, use the following GitHub CLI command.

> **Note**: This command assumes you are in an online environment. If you are in an offline or air-gapped environment, see [Verifying attestations offline](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/verify-attestations-offline).

```bash
gh attestation verify PATH/TO/RAKURAI/SCHEDULER/BINARY -R rakurai-io/rakurai-validator
```

### Results

Check the command output to determine the result. If verification is successful, you will see a message like `✓ Verification succeeded!`. If verification fails, you will see an error message such as `✗ Loading attestations from GitHub API failed`.

#### Successful verification

You’ll see the enforced policy, a checkmark, and one or more matched attestations summarizing the build and signer. Example:

```bash
$ gh attestation verify ./PATH/TO/BINARY -R rakurai-io/rakurai-validator
Loaded digest sha256:<ARTIFACT_DIGEST> for file://...
Loaded <N> attestations from GitHub API

Policy criteria enforced:
- Predicate type: https://slsa.dev/provenance/v1
- Source repository owner: https://github.com/rakurai-io
- Source repository: https://github.com/rakurai-io/rakurai-validator
- Subject Alternative Name: ^https://github.com/rakurai-io/rakurai-validator/
- OIDC issuer: https://token.actions.githubusercontent.com

✓ Verification succeeded!

Matched attestations:
- Attestation #<NUMBER>
- Build repo: rakurai-io/rakurai-validator
- Build workflow: .github/workflows/<WORKFLOW_FILE>@refs/tags/<TAG>
- Signer repo: rakurai-io/rakurai-validator
- Signer workflow: .github/workflows/<WORKFLOW_FILE>@refs/tags/<TAG>
```

#### Failed verification

A failure result means no attestation could be found (HTTP 404), and the binary’s authenticity could not be confirmed. Example:

```bash
$ gh attestation verify ./PATH/TO/BINARY -R rakurai-io/rakurai-validator
Loaded digest sha256:<artifact-digest> for file://...
✗ Loading attestations from GitHub API failed

Error: HTTP 404: Not Found
```

## Verify SBOM attestation with the GitHub CLI

The scheduler binary is accompanied by a Software Bill of Materials (SBOM) and attested using GitHub SBOM attestation. The following steps show how to verify it using the GitHub CLI.

### Prerequisites

- [GitHub CLI](https://github.com/cli/cli#installation) installed

### Verification

To verify the Rakurai scheduler SBOM attestation, use the following GitHub CLI command.

```bash
gh attestation verify PATH/TO/RAKURAI/SCHEDULER/BINARY -R rakurai-io/rakurai-validator --predicate-type https://spdx.dev/Document/v2.3
```

To view more information on the attestation, reference the --format json flag.

```bash
gh attestation verify PATH/TO/RAKURAI/SCHEDULER/BINARY -R rakurai-io/rakurai-validator --predicate-type https://spdx.dev/Document/v2.3 --format json --jq '.[].verificationResult.statement.predicate'
```

## Verify Rakurai scheduler binary checksum

A signed checksum of the scheduler binary is included alongside the release.

### Verification

- Copy the scheduler signing key, available in the [FAQs](https://rakurai.io/faqs) and save it to `public-key.asc`

- Import the public key
```bash
gpg --import public-key.asc
```

- Verify the checksum signature
```bash
gpg --verify rakurai_scheduler.sha512.asc rakurai_scheduler.sha512
```
If the output shows `Good signature`, the checksum is authentic.
If the output shows `BAD signature`, the checksum is invalid.

- Verify binary matches checksum
```bash
sha512sum -c rakurai_scheduler.sha512
```
If the output shows `OK`, the binary matches checksum.
If he output shows `FAILED`, the binary does not match the checksum.

## Scheduler I/O Isolation Test
The Scheduler I/O Isolation test setup is a test harness for the Rakurai scheduler that verifies the scheduler does not perform any file I/O or network I/O syscalls. The test applies seccomp filters to disable these syscalls before running the scheduler. If the scheduler attempts to perform file or network operations, the process will be terminated, proving that the scheduler operates without requiring these capabilities. The test also verifies that transactions flow correctly through the scheduler by tracking sent and received transaction signatures, ensuring the scheduler functions properly even with these syscall restrictions in place. This test runs automatically in the release CI/CD pipeline, ensuring that all published scheduler binaries are verified to operate safely under I/O restrictions.

- Run the test
```bash
./target/release/scheduler-test-setup
```