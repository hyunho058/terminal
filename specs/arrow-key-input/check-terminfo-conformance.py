#!/usr/bin/env python3
"""Independent conformance check for the arrow-key-input byte table.

WHY THIS EXISTS: the ``KeySeq`` table in ``crates/app/src/input.rs`` and its unit
tests were written by the same author, so "the tests pass" only proves internal
consistency. ``terminfo`` is an EXTERNAL oracle — it is what zsh and every TUI
actually read — so this diffs our table against ``infocmp xterm-256color``, the
TERM the PTY sets (``crates/term-core/src/pty.rs``).

Home/End are EXPECTED to differ. terminfo says ``khome=\\EOH``/``kend=\\EOF``, but
those presume ``smkx`` has been applied and zsh never sends ``smkx`` (measured),
which leaves ``^[[H``/``^[[F`` bound to nothing in its keymap. Spec decision D11
chose ``ESC[1~``/``ESC[4~`` — the forms zsh actually binds. That deviation is
asserted rather than tolerated: drifting back to the terminfo form fails here.

Exit 0 = conformant, 1 = drifted, 2 = oracle unavailable.
"""

import pathlib
import re
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
INPUT_RS = ROOT / "crates/app/src/input.rs"
TERMINAL_APP_RS = ROOT / "crates/app/src/terminal_app.rs"

# NamedKey::ArrowUp => KeySeq::Cursor(b'A'),   |   NamedKey::F5 => KeySeq::Tilde(15),
ENTRY = re.compile(
    r"^\s*NamedKey::(\w+)\s*=>\s*KeySeq::(Cursor|Ss3|Tilde)\(\s*(?:b'(.)'|(\d+))\s*\)",
    re.M,
)


def parse_table(src: str) -> dict[str, tuple[str, str]]:
    """key -> (variant, argument-as-text)."""
    return {m[1]: (m[2], m[3] or m[4]) for m in ENTRY.finditer(src)}


def unmodified_bytes(variant: str, arg: str) -> bytes:
    """Bytes the entry emits with no modifiers.

    Cursor under DECCKM and Ss3 are both ``ESC O x`` — exactly what terminfo's
    kcuu1/kf1 encode, since those entries presume smkx.
    """
    if variant in ("Cursor", "Ss3"):
        return b"\x1bO" + arg.encode()
    return b"\x1b[" + arg.encode() + b"~"


def terminfo() -> dict[str, bytes]:
    out = subprocess.run(
        ["infocmp", "-1", "xterm-256color"], capture_output=True, text=True, check=True
    ).stdout
    caps: dict[str, bytes] = {}
    for name, value in re.findall(r"^\s*(\w+)=(.*),$", out, re.M):
        caps[name] = value.replace("\\E", "\x1b").encode()
    return caps


def show(raw: bytes) -> str:
    return raw.decode("latin-1").replace("\x1b", "^[")


def check_wiring() -> int:
    """Assert the app actually reads DECCKM from the active tab and passes it on (R7.2).

    WHY: the byte table above is only half the feature. The other half is one line in
    ``terminal_app.rs`` — and it has NO automated coverage, because ``Running::on_key``
    needs a window + GPU + PTY and is never constructed in a test. Verified empirically:
    replacing that read with ``let app_cursor = false;`` leaves the entire 123-test suite,
    clippy, AND the byte-table check above all green while silently deleting every
    application-mode arrow. This static assertion is the cheap guard against exactly that
    regression. It is not a substitute for the live check in the R7.2 handback.

    Returns the number of failures.
    """
    src = TERMINAL_APP_RS.read_text()
    failures = 0
    print("\nWiring (R7.2) — the byte table is inert unless the app reads the live mode:")

    # The mode must come from the ACTIVE tab's terminal, not a literal or a global.
    read = re.search(r"let\s+(\w+)\s*=\s*self\.tabs\.active\(\)\.terminal\.app_cursor_keys\(\)", src)
    if read:
        print(f"  ok   reads per-tab DECCKM into `{read[1]}` (D7 — per-session state)")
    else:
        print("  FAIL terminal_app.rs never reads self.tabs.active().terminal.app_cursor_keys()")
        failures += 1

    # …and must be handed to encode_key as the 4th argument, not dropped.
    call = re.search(r"encode_key\(\s*&input\.key,\s*input\.text\.as_deref\(\),\s*input\.mods,\s*(\w+)\s*\)", src)
    if not call:
        print("  FAIL encode_key is not called with (&key, text, input.mods, <mode>)")
        failures += 1
    elif read and call[1] != read[1]:
        print(f"  FAIL encode_key receives `{call[1]}`, but the mode was read into `{read[1]}`")
        failures += 1
    elif call[1] in ("true", "false"):
        print(f"  FAIL encode_key receives the literal `{call[1]}` — DECCKM is not wired")
        failures += 1
    else:
        print(f"  ok   passes `{call[1]}` + input.mods to encode_key")

    return failures


def main() -> int:
    if not shutil.which("infocmp"):
        print("SKIP: infocmp unavailable — external oracle cannot run", file=sys.stderr)
        return 2

    table = parse_table(INPUT_RS.read_text())
    if not table:
        print(f"FAIL: parsed no KeySeq entries from {INPUT_RS}", file=sys.stderr)
        return 1

    caps = terminfo()
    failures = 0

    # (our NamedKey, terminfo capability) — every key terminfo and D11 agree on.
    agreed = [
        ("ArrowUp", "kcuu1"), ("ArrowDown", "kcud1"),
        ("ArrowRight", "kcuf1"), ("ArrowLeft", "kcub1"),
        ("Insert", "kich1"), ("Delete", "kdch1"),
        ("PageUp", "kpp"), ("PageDown", "knp"),
    ] + [(f"F{n}", f"kf{n}") for n in range(1, 13)]

    print(f"Byte table vs `infocmp xterm-256color`  ({len(agreed)} keys):")
    for key, cap in agreed:
        if key not in table:
            print(f"  FAIL {key:<11} missing from the KeySeq table")
            failures += 1
            continue
        if cap not in caps:
            print(f"  FAIL {key:<11} terminfo has no {cap}")
            failures += 1
            continue
        got, want = unmodified_bytes(*table[key]), caps[cap]
        if got == want:
            print(f"  ok   {key:<11} {show(got)}")
        else:
            print(f"  FAIL {key:<11} terminfo {cap}={show(want)}, table emits {show(got)}")
            failures += 1

    print("\nDeliberate deviation (D11 — zsh binds these; terminfo's forms are dead at the prompt):")
    for key, want_arg, cap in [("Home", "1", "khome"), ("End", "4", "kend")]:
        got = table.get(key)
        if got == ("Tilde", want_arg):
            print(
                f"  ok   {key:<11} {show(unmodified_bytes(*got))}"
                f"   (terminfo {cap}={show(caps.get(cap, b''))} deliberately unused)"
            )
        else:
            print(f"  FAIL {key:<11} expected Tilde({want_arg}), table has {got}")
            failures += 1

    failures += check_wiring()

    if failures:
        print(f"\nFAIL: {failures} check(s) drifted.", file=sys.stderr)
        return 1
    print("\nPASS: byte table conforms to the external oracle, and the wiring is live.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
