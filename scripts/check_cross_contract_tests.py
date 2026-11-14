#!/usr/bin/env python3
"""Fail if any contract calls into another contract's address without a
same-Env integration test covering that call path.

This is the actual enforcement issue #62 asked for ("when any contract
gains a call into another contract's address, a same-Env, two-contract
integration test must land in the same PR") after it turned out to have
been purely a documented convention with nothing checking it: #62's own
evidence method ("no contract's Cargo.toml depends on another") does not
detect a Soroban cross-contract call at all, since those need only a
runtime `Address`, never a Cargo dependency edge. `payment`, `treasury`,
and `channel` all gained a real cross-contract call into `compliance`
without anyone adding the test this rule calls for (issue #254) -- because
nothing actually ran this rule.

Method:
1. For each contracts/*/src/lib.rs, find its own `#[contract] pub struct
   XContract;` and every `#[contractclient(name = "YClient")]` trait it
   declares.
2. For each such YClient, check whether `YClient::new(` is called anywhere
   in that file OUTSIDE a `#[cfg(test)]` module -- i.e. in real contract
   logic, not test setup. If so, this contract has a production
   cross-contract call into whatever contract YClient targets (by
   convention, "YContract": `ComplianceClient` -> `ComplianceContract`).
3. For every (caller, target) pair found this way, search every test file
   in the whole repo for one that both contains a `#[test]` function and
   calls `.register(` on both the caller's and the target's contract
   struct somewhere in that file -- i.e. an actual same-Env integration
   test (this codebase's convention is a shared `setup()` helper called
   from multiple `#[test]` functions, so the registrations and the test
   function are usually in different functions within one file; checking
   per-file rather than per-test-function-body matches that convention).
   Fail if no such file exists.

This intentionally does not flag `token::*Client` (the SEP-41 token
contract -- already documented as the one sanctioned external call, not a
sibling contract in this workspace) or a contract's own generated
`XContractClient` (same-contract test client, not a cross-contract call).
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

CONTRACT_STRUCT = re.compile(r"#\[contract\]\s*\n\s*pub struct (\w+);")
CONTRACTCLIENT = re.compile(r'#\[contractclient\(name\s*=\s*"(\w+)"\)\]')
CLIENT_NEW = re.compile(r"\b(\w+Client)::new\(")
TEST_FN = re.compile(r"#\[test\]\s*\n\s*fn (\w+)\s*\(")
REGISTER_CALL = re.compile(r"\.register\(\s*(\w+)\b")

# Not cross-contract calls within this workspace.
IGNORED_CLIENT_PREFIXES = ("Token", "StellarAsset")


def strip_cfg_test_blocks(text):
    """Return `text` with every `#[cfg(test)] mod ... { ... }` block
    removed, so callers only see production code. Uses brace counting like
    check_pub_fn_auth.py's fn_body(), since lib.rs files are not trivially
    line-regex-separable into test/non-test regions."""
    out = []
    i = 0
    pattern = re.compile(r"#\[cfg\(test\)\]\s*\n\s*mod\s+\w+\s*\{")
    while True:
        m = pattern.search(text, i)
        if not m:
            out.append(text[i:])
            break
        out.append(text[i:m.start()])
        depth = 0
        j = m.end() - 1  # at the '{'
        while j < len(text):
            if text[j] == "{":
                depth += 1
            elif text[j] == "}":
                depth -= 1
                if depth == 0:
                    j += 1
                    break
            j += 1
        i = j
    return "".join(out)


def find_production_cross_contract_calls():
    """Return a sorted list of (caller_struct, target_struct, file, client_name)."""
    calls = []
    for path in sorted(ROOT.glob("contracts/*/src/lib.rs")):
        text = path.read_text()
        struct_match = CONTRACT_STRUCT.search(text)
        if not struct_match:
            continue
        caller = struct_match.group(1)

        client_names = set(CONTRACTCLIENT.findall(text))
        if not client_names:
            continue

        production_text = strip_cfg_test_blocks(text)
        for client_name in sorted(client_names):
            if client_name.startswith(IGNORED_CLIENT_PREFIXES):
                continue
            if re.search(rf"\b{re.escape(client_name)}::new\(", production_text):
                if not client_name.endswith("Client"):
                    continue
                target = client_name[: -len("Client")] + "Contract"
                calls.append((caller, target, str(path.relative_to(ROOT)), client_name))
    return calls


def find_candidate_test_files():
    """Return the text of every .rs file under contracts/*/src that
    contains at least one `#[test]` function -- i.e. is actually a test
    file, not just a file that happens to mention a contract struct name."""
    texts = []
    for path in sorted(ROOT.glob("contracts/*/src/**/*.rs")):
        text = path.read_text()
        if TEST_FN.search(text):
            texts.append(text)
    return texts


def main():
    required = find_production_cross_contract_calls()
    if not required:
        print("no cross-contract calls found under contracts/*/src/lib.rs; nothing to check")
        return 0

    test_files = find_candidate_test_files()

    failures = []
    for caller, target, file, client_name in required:
        covered = any(
            re.search(rf"\.register\(\s*{re.escape(caller)}\b", text)
            and re.search(rf"\.register\(\s*{re.escape(target)}\b", text)
            for text in test_files
        )
        if not covered:
            failures.append(
                f"{file}: {caller} calls {client_name} (into {target}) with no same-Env "
                f"integration test file registering both {caller} and {target} "
                f"(issue #62/#254 -- see docs/architecture.md)"
            )

    for failure in failures:
        print(failure, file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
