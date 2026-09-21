#!/usr/bin/env python3
"""
tx_log_analyzer.py

Analyze Solana transaction logs for tx_in_signature and tx_out_signature.

The script:
1. Reads all matching files (tx_io.log, tx_io.log.1, tx_io.log.2, ...).
2. Combines all tx_in_signature and tx_out_signature occurrences.
3. Counts total tx_in_signature and tx_out_signature occurrences.
4. Compares signatures as multisets (counting duplicates).
   - Reports how many occurrences are present in both.
   - Reports how many occurrences are only in tx_in.
   - Reports how many occurrences are only in tx_out.

Usage:
    python tx_log_analyzer.py [logpattern]

Arguments:
    logpattern   Glob pattern for logs (default: tx_io.log*)
"""

import re
import argparse
import glob
from collections import Counter
import math

# Regex patterns to extract signatures
in_pattern = re.compile(r"tx_in_signature:\s+(\S+)")
out_pattern = re.compile(r"tx_out_signature:\s+(\S+)")

def analyze_logs(log_pattern: str):
    """
    Analyze multiple log files for tx_in_signature and tx_out_signature.

    Args:
        log_pattern (str): Glob pattern for log files (e.g., tx_io.log*).

    Prints:
        - Counts of tx_in_signature and tx_out_signature.
        - Which type is more frequent.
        - Multiset comparison: counts in both, only in tx_in, only in tx_out.
    """
    tx_in = []
    tx_out = []

    # Expand glob and sort files for deterministic order
    files = sorted(glob.glob(log_pattern))
    if not files:
        print(f"No log files found matching: {log_pattern}")
        return

    print(f"Analyzing log files: {', '.join(files)}\n")

    # Parse files
    for file_path in files:
        with open(file_path, "r") as f:
            for line in f:
                if match := in_pattern.search(line):
                    tx_in.append(match.group(1))
                elif match := out_pattern.search(line):
                    tx_out.append(match.group(1))

    # Total counts
    in_count = len(tx_in)
    out_count = len(tx_out)

    print("Summary:")
    print(f"  tx_in_signature count : {in_count}")
    print(f"  tx_out_signature count: {out_count}")
    if in_count > out_count:
        print("  More tx_in_signatures than tx_out_signatures")
    elif out_count > in_count:
        print("  More tx_out_signatures than tx_in_signatures")
    else:
        print("  Counts are equal")

    # Multiset comparison (counts with duplicates)
    tx_in_counter = Counter(tx_in)
    tx_out_counter = Counter(tx_out)

    # Compute overlap and differences
    both = 0
    only_in = 0
    only_out = 0

    only_in_sigs = []
    only_out_sigs = []

    all_sigs = set(tx_in_counter.keys()) | set(tx_out_counter.keys())
    for sig in all_sigs:
        in_count = tx_in_counter.get(sig, 0)
        out_count = tx_out_counter.get(sig, 0)
        both += min(in_count, out_count)
        only_in += max(in_count - out_count, 0)
        only_out += max(out_count - in_count, 0)
        if in_count - out_count > 0 :
            only_in_sigs.append(sig)
        if out_count - in_count > 0:
            only_out_sigs.append(sig)

    
         # ## for debug purposes only
        # if in_count - out_count > 0 or out_count - in_count > 0:
        #     # Grep the sig across all files and print matching lines
        #     print(f"================================")
        #     for file_path in files:
        #         with open(file_path, "r") as f:
        #             for line in f:
        #                 if sig in line:
        #                     print(f"  {file_path}: {line.strip()}")

    ## logic to match the last tx_out with the last tx_in
    # if len(tx_in) > len(tx_out):
    #     if tx_out:
    #         # Get the last tx_out signature in the list
    #         last_tx_out = tx_out[-1]

    #         try:
    #             # Find the last occurrence of this tx_out signature in tx_in
    #             # We reverse tx_in to find the last match easily
    #             reversed_index = tx_in[::-1].index(last_tx_out)

    #             # Convert the reversed index back to the original list index
    #             last_matched_idx = len(tx_in) - 1 - reversed_index

    #             # Count how many tx_in signatures occur **after** the last matched tx_out
    #             remaining_in = len(tx_in) - (last_matched_idx + 1)

    #             # Print the number of extra tx_in signatures after the last matched tx_out
    #             print(f"\nExtra tx_in after last matched tx_out: {remaining_in}")

    #         except ValueError:
    #             # This exception occurs if the last tx_out signature is not found in tx_in
    #             print("\nNo matching tx_out signatures found in tx_in")


    print("\nComparison (multiset-aware):")
    print(f"  Present in both       : {both}")
    print(f"  Only in tx_in         : {only_in}")
    print(f"  Only in tx_out        : {only_out}")


    detect_excess_tx_in_percentile(tx_in, tx_out)

    print("\nDetails:")

    if only_in_sigs:
        print("\nSignatures only in tx_in:")
        for sig in only_in_sigs:
            print(f"  {sig}")

    if only_out_sigs:
        print("\nSignatures only in tx_out:")
        for sig in only_out_sigs:
            print(f"  {sig}")



def detect_excess_tx_in_percentile(tx_in, tx_out):
    """
    Detect excessive tx_in signatures towards the end and compute
    what percentage of the file (from bottom) contains the unmatched tx_in.

    Args:
        tx_in (list): List of tx_in_signature in order.
        tx_out (list): List of tx_out_signature in order.
    """
    if not tx_in:
        print("No tx_in signatures to analyze.")
        return

    from collections import Counter

    # Counter for tx_out for matching
    tx_out_counter = Counter(tx_out)

    # List to store indices of unmatched tx_in
    unmatched_indices = []

    # Iterate from the end of tx_in backward
    for idx in reversed(range(len(tx_in))):
        sig = tx_in[idx]
        if tx_out_counter[sig] > 0:
            tx_out_counter[sig] -= 1
        else:
            unmatched_indices.append(idx)

    if unmatched_indices:
        # unmatched_indices are in reverse order (from end)
        last_unmatched_idx = min(unmatched_indices)  # closest to the top
        percent_from_bottom = 100 * (len(tx_in) - last_unmatched_idx) / len(tx_in)
        print(f"\nExcessive unmatched tx_in detected:")
        print(f"  Number of unmatched tx_in: {len(unmatched_indices)}")
        print(f"  They occupy the last {percent_from_bottom:.2f}% of the tx_in list")
    else:
        print("\nNo excessive unmatched tx_in detected at the end.")


def main():
    parser = argparse.ArgumentParser(
        description="Analyze Solana transaction logs (across multiple files) for tx_in_signature and tx_out_signature, counting duplicates."
    )
    parser.add_argument(
        "logpattern",
        nargs="?",
        default="tx_io.log*",
        help="Glob pattern for log files (default: tx_io.log*)",
    )

    args = parser.parse_args()
    analyze_logs(args.logpattern)

if __name__ == "__main__":
    main()
