#!/usr/bin/env python3
"""apply_patches.py — apply Mirage native Chromium patches onto a checkout.

Mirage patch templates (*.patch, see patches/README.md) are context-anchored
replacement blocks, not line-number diffs. For every block this script:

  1. resolves the block's MIRAGE-SIGNATURE regex to exactly one location —
     a pinned MIRAGE-FILE, or a tree-wide search when no file is pinned /
     the pinned file is missing or does not match;
  2. verifies the MIRAGE-REPLACE-FROM text occurs exactly once there
     (LF / no-EOL / CRLF line-ending variants are tried in that order), or
     recognizes the block as already applied via its REPLACE-TO text;
  3. replaces it verbatim with MIRAGE-REPLACE-TO, backing the pristine file
     up to <path>.mirage-orig on first modification.

Zero or multiple matches are REFUSALS with full diagnostics (which files,
which counts, code context around the match) — the applier never guesses.
MIRAGE-MANUAL blocks are reported as manual actions with their instructions.

Usage:
  python apply_patches.py --chromium-dir C:\\chromium\\src --dry-run
  python apply_patches.py --chromium-dir C:\\chromium\\src
  python apply_patches.py --chromium-dir C:\\chromium\\src --revert

Exit codes: 0 = no errors, 1 = at least one block errored, 2 = bad usage.

Python 3.8+, standard library only.
"""

import argparse
import os
import re
import sys

BACKUP_SUFFIX = ".mirage-orig"
PATCH_SUFFIX = ".patch"

# Directories never searched (VCS data, build output, caches, toolchains).
SKIP_DIR_NAMES = frozenset({
    ".git", ".hg", ".svn", ".cipd", ".cache", "node_modules", "__pycache__",
})
# gn output directories: out, out_Default, out/Release, ...
SKIP_DIR_PREFIXES = ("out",)

# Signature search touches text sources only; larger files are skipped
# (generated blobs, bundled data).
MAX_FILE_BYTES = 8 * 1024 * 1024

TEXT_SUFFIXES = frozenset({
    ".cc", ".cpp", ".cxx", ".c", ".h", ".hh", ".hpp", ".hxx", ".inl", ".inc",
    ".mm", ".m", ".idl", ".js", ".json", ".json5", ".mojom", ".gn", ".gni",
    ".py", ".txt", ".md", ".html", ".htm", ".css", ".yml", ".yaml", ".sh",
    ".bat", ".cmd", ".stamp", ".sig", ".owner",
})

STATUS_APPLIED = "applied"
STATUS_WOULD_APPLY = "would-apply"
STATUS_ALREADY = "already-applied"
STATUS_MANUAL = "manual"
STATUS_ERROR = "error"

_CONTEXT_BEFORE = 6
_CONTEXT_AFTER = 20
_MAX_LISTED_MATCHES = 15


class PatchFormatError(Exception):
    """A .patch file violates the Mirage patch format."""


class BlockError(Exception):
    """A block could not be resolved or applied (refusal, not a crash)."""


class Block(object):
    """One MIRAGE-BLOCK / MIRAGE-MANUAL section of a patch file."""

    def __init__(self, kind, patch_id, index, path):
        self.kind = kind              # "replace" | "manual"
        self.patch_id = patch_id
        self.index = index
        self.path = path
        self.file = None              # pinned relative path, or None
        self.signature = None         # regex source text
        self.verify = []              # notes echoed in the report
        self.replace_from = None      # list of lines
        self.replace_to = None        # list of lines
        self.instructions = None      # list of lines (manual blocks)


class Patch(object):
    """One parsed .patch file."""

    def __init__(self, path):
        self.path = path
        self.patch_id = None
        self.title = ""
        self.target = ""
        self.descriptions = []
        self.blocks = []


def out(message=""):
    """Print that never dies on a narrow console codepage."""
    try:
        print(message)
    except UnicodeEncodeError:
        enc = sys.stdout.encoding or "ascii"
        print(message.encode(enc, "backslashreplace").decode(enc))


def force_utf8_stdio():
    """Patch data contains UTF-8 (em-dashes etc.); keep console output sane."""
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="backslashreplace")
        except (AttributeError, ValueError):
            pass


def read_text(path):
    """Byte-preserving UTF-8 round-trip (surrogateescape keeps odd bytes)."""
    with open(path, "rb") as fh:
        return fh.read().decode("utf-8", errors="surrogateescape")


def write_text(path, text):
    with open(path, "wb") as fh:
        fh.write(text.encode("utf-8", errors="surrogateescape"))


# ---------------------------------------------------------------------------
# Patch file parsing
# ---------------------------------------------------------------------------

def _strip_blank_edges(lines):
    """Drop leading/trailing blank lines from captured block text."""
    start, end = 0, len(lines)
    while start < end and lines[start].strip() == "":
        start += 1
    while end > start and lines[end - 1].strip() == "":
        end -= 1
    return lines[start:end]


def _validate_replace_block(block, path, lineno):
    if not block.signature:
        raise PatchFormatError(
            "%s:%d: replace block %d has no MIRAGE-SIGNATURE" %
            (path, lineno, block.index))
    if not block.replace_from:
        raise PatchFormatError(
            "%s:%d: replace block %d has empty MIRAGE-REPLACE-FROM" %
            (path, lineno, block.index))
    if block.replace_to is None:
        raise PatchFormatError(
            "%s:%d: replace block %d has no MIRAGE-REPLACE-TO" %
            (path, lineno, block.index))
    if "\n".join(block.replace_from) == "\n".join(block.replace_to):
        raise PatchFormatError(
            "%s:%d: replace block %d FROM and TO are identical" %
            (path, lineno, block.index))


def _validate_manual_block(block, path, lineno):
    if not block.instructions:
        raise PatchFormatError(
            "%s:%d: manual block %d has empty MIRAGE-INSTRUCTIONS" %
            (path, lineno, block.index))


def parse_patch_file(path):
    """Parse one .patch file into a Patch. Raises PatchFormatError."""
    patch = Patch(path)
    with open(path, "r", encoding="utf-8") as fh:
        lines = fh.read().splitlines()

    mode = "header"        # header | block | between | from | to | instructions
    block = None
    captured = []
    block_count = 0

    for lineno, raw in enumerate(lines, start=1):
        line = raw.rstrip("\r")

        def fail(message):
            raise PatchFormatError("%s:%d: %s" % (path, lineno, message))

        # --- text capture modes: only the exact closing directive ends them
        if mode == "from":
            if line.strip() == "MIRAGE-REPLACE-TO:":
                block.replace_from = _strip_blank_edges(captured)
                captured = []
                mode = "to"
            else:
                if line.startswith("MIRAGE-"):
                    fail("directive %r inside REPLACE-FROM text" % line[:40])
                captured.append(line)
            continue

        if mode == "to":
            if line.strip() == "MIRAGE-ENDBLOCK":
                block.replace_to = _strip_blank_edges(captured)
                captured = []
                _validate_replace_block(block, path, lineno)
                mode = "between"
            else:
                if line.startswith("MIRAGE-"):
                    fail("directive %r inside REPLACE-TO text" % line[:40])
                captured.append(line)
            continue

        if mode == "instructions":
            if line.strip() == "MIRAGE-ENDBLOCK":
                block.instructions = _strip_blank_edges(captured)
                captured = []
                _validate_manual_block(block, path, lineno)
                mode = "between"
            else:
                if line.startswith("MIRAGE-"):
                    fail("directive %r inside INSTRUCTIONS text" % line[:40])
                captured.append(line)
            continue

        # --- header, between-blocks and block-directive modes
        if not line or line.startswith("#"):
            continue

        if mode == "header":
            if line.startswith("MIRAGE-PATCH:"):
                if patch.patch_id is not None:
                    fail("duplicate MIRAGE-PATCH")
                patch.patch_id = line[len("MIRAGE-PATCH:"):].strip()
            elif line.startswith("MIRAGE-TITLE:"):
                patch.title = line[len("MIRAGE-TITLE:"):].strip()
            elif line.startswith("MIRAGE-TARGET:"):
                patch.target = line[len("MIRAGE-TARGET:"):].strip()
            elif line.startswith("MIRAGE-DESCRIPTION:"):
                patch.descriptions.append(
                    line[len("MIRAGE-DESCRIPTION:"):].strip())
            elif line == "MIRAGE-BLOCK":
                block_count += 1
                block = Block("replace", patch.patch_id, block_count, path)
                patch.blocks.append(block)
                mode = "block"
            elif line == "MIRAGE-MANUAL":
                block_count += 1
                block = Block("manual", patch.patch_id, block_count, path)
                patch.blocks.append(block)
                mode = "block"
            else:
                fail("unknown directive in header: %r" % line[:60])
            continue

        if mode == "between":
            if line == "MIRAGE-BLOCK":
                block_count += 1
                block = Block("replace", patch.patch_id, block_count, path)
                patch.blocks.append(block)
                mode = "block"
            elif line == "MIRAGE-MANUAL":
                block_count += 1
                block = Block("manual", patch.patch_id, block_count, path)
                patch.blocks.append(block)
                mode = "block"
            else:
                fail("expected MIRAGE-BLOCK or MIRAGE-MANUAL after "
                     "MIRAGE-ENDBLOCK, got: %r" % line[:60])
            continue

        if mode == "block":
            if line.startswith("MIRAGE-FILE:"):
                block.file = line[len("MIRAGE-FILE:"):].strip()
            elif line.startswith("MIRAGE-SIGNATURE:"):
                block.signature = line[len("MIRAGE-SIGNATURE:"):].strip()
            elif line.startswith("MIRAGE-VERIFY:"):
                block.verify.append(line[len("MIRAGE-VERIFY:"):].strip())
            elif line.strip() == "MIRAGE-REPLACE-FROM:":
                if block.kind != "replace":
                    fail("MIRAGE-REPLACE-FROM is only valid in MIRAGE-BLOCK")
                captured = []
                mode = "from"
            elif line.strip() == "MIRAGE-INSTRUCTIONS:":
                if block.kind != "manual":
                    fail("MIRAGE-INSTRUCTIONS is only valid in MIRAGE-MANUAL")
                captured = []
                mode = "instructions"
            elif line in ("MIRAGE-BLOCK", "MIRAGE-MANUAL"):
                fail("previous block was not closed with MIRAGE-ENDBLOCK")
            else:
                fail("unknown directive inside block: %r" % line[:60])
            continue

        fail("unreachable parser state: %r" % mode)

    if mode == "from":
        raise PatchFormatError(
            "%s: REPLACE-FROM text not closed with MIRAGE-REPLACE-TO:" % path)
    if mode == "to":
        raise PatchFormatError(
            "%s: REPLACE-TO text not closed with MIRAGE-ENDBLOCK" % path)
    if mode == "instructions":
        raise PatchFormatError(
            "%s: INSTRUCTIONS text not closed with MIRAGE-ENDBLOCK" % path)
    if mode == "block":
        raise PatchFormatError(
            "%s: block %d has directives but no REPLACE-FROM/INSTRUCTIONS "
            "section" % (path, block.index))
    if patch.patch_id is None:
        raise PatchFormatError("%s: missing MIRAGE-PATCH header" % path)
    if not patch.title:
        raise PatchFormatError("%s: missing MIRAGE-TITLE header" % path)
    if not patch.target:
        raise PatchFormatError("%s: missing MIRAGE-TARGET header" % path)
    return patch


# ---------------------------------------------------------------------------
# Filesystem search
# ---------------------------------------------------------------------------

def walk_files(root):
    """Deterministic walk, pruning VCS/build/cache directories."""
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(
            d for d in dirnames
            if d not in SKIP_DIR_NAMES and not d.startswith(SKIP_DIR_PREFIXES)
        )
        for name in sorted(filenames):
            yield os.path.join(dirpath, name)


def iter_source_files(root):
    """Text source files eligible for signature search."""
    for path in walk_files(root):
        name = os.path.basename(path)
        if os.path.splitext(name)[1].lower() not in TEXT_SUFFIXES:
            continue
        try:
            if os.path.getsize(path) > MAX_FILE_BYTES:
                continue
        except OSError:
            continue
        yield path


def tree_search(root, regex):
    """All (path, match_count) for the signature under root."""
    matches = []
    for path in iter_source_files(root):
        try:
            content = read_text(path)
        except OSError:
            continue
        count = len(regex.findall(content))
        if count:
            matches.append((path, count))
    return matches


def resolve_block_file(block, regex, root, notes):
    """Exactly one file must carry the signature exactly once."""
    if block.file:
        pinned = os.path.normpath(
            os.path.join(root, *block.file.split("/")))
        if os.path.isfile(pinned):
            content = read_text(pinned)
            count = len(regex.findall(content))
            if count == 1:
                return pinned
            if count == 0:
                notes.append(
                    "pinned file %s carries no signature match; "
                    "falling back to tree search" % block.file)
            else:
                raise BlockError(
                    "signature matches %d times inside pinned file %s; "
                    "tighten MIRAGE-SIGNATURE" % (count, block.file))
        else:
            notes.append(
                "pinned file %s not found; falling back to tree search" %
                block.file)

    matches = tree_search(root, regex)
    total = sum(count for _, count in matches)
    if total == 0:
        raise BlockError(
            "signature not found anywhere under %s" % root)
    if total == 1:
        return matches[0][0]

    listing = "; ".join(
        "%s (%d)" % (os.path.relpath(p, root), c)
        for p, c in matches[:_MAX_LISTED_MATCHES])
    more = ""
    if len(matches) > _MAX_LISTED_MATCHES:
        more = "; +%d more files" % (len(matches) - _MAX_LISTED_MATCHES)
    raise BlockError(
        "signature ambiguous: %d matches across %d files: %s%s"
        % (total, len(matches), listing, more))


def context_around(content, regex, rel_file):
    """Code context around the signature match, for refusal diagnostics."""
    match = regex.search(content)
    if match is None:
        return "      (signature not present in the resolved file)"
    lines = content.split("\n")
    line_no = content.count("\n", 0, match.start()) + 1
    lo = max(0, line_no - 1 - _CONTEXT_BEFORE)
    hi = min(len(lines), line_no + _CONTEXT_AFTER)
    buf = ["      %s:%d - signature match context:" % (rel_file, line_no)]
    for i in range(lo, hi):
        marker = ">>" if i == line_no - 1 else "  "
        buf.append("      %s %5d | %s" % (marker, i + 1, lines[i][:200]))
    return "\n".join(buf)


# ---------------------------------------------------------------------------
# Block processing
# ---------------------------------------------------------------------------

def from_to_variants(from_lines, to_lines):
    """(from, to) exact-text pairs for the supported line-ending styles."""
    f_lf = "\n".join(from_lines)
    t_lf = "\n".join(to_lines) if to_lines is not None else ""
    f_crlf = f_lf.replace("\n", "\r\n")
    t_crlf = t_lf.replace("\n", "\r\n")
    return [
        (f_lf + "\n", t_lf + "\n"),
        (f_lf, t_lf),
        (f_crlf + "\r\n", t_crlf + "\r\n"),
        (f_crlf, t_crlf),
    ]


def ensure_backup(path):
    """Keep the pristine copy; returns True when a new backup was created."""
    backup = path + BACKUP_SUFFIX
    if os.path.exists(backup):
        return False
    with open(path, "rb") as src:
        data = src.read()
    with open(backup, "wb") as dst:
        dst.write(data)
    return True


def process_replace_block(block, root, dry_run):
    notes = []
    result = {
        "patch_id": block.patch_id,
        "index": block.index,
        "kind": "replace",
        "file": None,
        "status": None,
        "detail": "",
        "notes": notes,
        "verify": block.verify,
        "context": None,
    }

    try:
        regex = re.compile(block.signature, re.MULTILINE)
    except re.error as exc:
        result["status"] = STATUS_ERROR
        result["detail"] = "invalid signature regex: %s" % exc
        return result

    try:
        target = resolve_block_file(block, regex, root, notes)
    except BlockError as exc:
        # Rename-style blocks replace their own anchor, so on a re-run the
        # signature no longer exists anywhere. If a file is pinned and the
        # replacement text is already present there, that is not an error.
        if block.file:
            pinned = os.path.normpath(os.path.join(root, *block.file.split("/")))
            if os.path.isfile(pinned):
                try:
                    pinned_content = read_text(pinned)
                except OSError:
                    pinned_content = None
                if pinned_content is not None:
                    for _, to_text in from_to_variants(
                            block.replace_from, block.replace_to):
                        if to_text and pinned_content.count(to_text) >= 1:
                            result["status"] = STATUS_ALREADY
                            result["file"] = block.file
                            result["detail"] = (
                                "REPLACE-TO already present "
                                "(rename-style block, signature consumed)")
                            return result
        result["status"] = STATUS_ERROR
        result["detail"] = str(exc)
        return result

    rel_target = os.path.relpath(target, root)
    result["file"] = rel_target
    content = read_text(target)

    matched = None
    for from_text, to_text in from_to_variants(
            block.replace_from, block.replace_to):
        # Already applied? For insertion-style blocks FROM is a prefix of TO
        # and stays present after the first run, so the TO check wins.
        if to_text and content.count(to_text) >= 1:
            result["status"] = STATUS_ALREADY
            result["detail"] = "REPLACE-TO already present"
            return result
        count = content.count(from_text)
        if count == 1:
            matched = (from_text, to_text)
            break
        if count > 1:
            result["status"] = STATUS_ERROR
            result["detail"] = ("REPLACE-FROM text occurs %d times in %s; "
                                "anchor is not unique" % (count, rel_target))
            return result

    if matched is None:
        result["status"] = STATUS_ERROR
        result["detail"] = (
            "REPLACE-FROM text not found in %s — signature matched, anchor "
            "text differs; verify against this tag (MIRAGE-VERIFY notes)"
            % rel_target)
        result["context"] = context_around(content, regex, rel_target)
        return result

    from_text, to_text = matched
    line_no = content.count("\n", 0, content.index(from_text)) + 1

    if dry_run:
        result["status"] = STATUS_WOULD_APPLY
        result["detail"] = "would replace at line %d" % line_no
        return result

    try:
        ensure_backup(target)
        new_content = content.replace(from_text, to_text, 1)
    except OSError as exc:
        result["status"] = STATUS_ERROR
        result["detail"] = "write failed: %s" % exc
        return result
    if new_content == content:
        result["status"] = STATUS_ERROR
        result["detail"] = "replacement produced no change (internal error)"
        return result
    write_text(target, new_content)
    result["status"] = STATUS_APPLIED
    result["detail"] = "replaced at line %d" % line_no
    return result


def process_manual_block(block, root):
    return {
        "patch_id": block.patch_id,
        "index": block.index,
        "kind": "manual",
        "file": block.file,
        "status": STATUS_MANUAL,
        "detail": "manual action required",
        "notes": [],
        "verify": block.verify,
        "instructions": block.instructions,
        "context": None,
    }


def print_block_result(res, verbose):
    file_part = res.get("file") or "-"
    line = "  block %-2d %-8s %-16s %s" % (
        res["index"], res["kind"], res["status"], file_part)
    if res.get("detail"):
        line += " — " + res["detail"]
    out(line)
    for note in res.get("notes", []):
        out("      note: %s" % note)
    if res["status"] == STATUS_ERROR and res.get("context"):
        out(res["context"])
    if verbose or res["status"] == STATUS_ERROR:
        for verify_note in res.get("verify", []):
            out("      verify: %s" % verify_note)
    if res["status"] == STATUS_MANUAL:
        for verify_note in res.get("verify", []):
            out("      verify: %s" % verify_note)
        for ins_line in res.get("instructions", []):
            out("      | %s" % ins_line)


# ---------------------------------------------------------------------------
# Revert
# ---------------------------------------------------------------------------

def do_revert(root, dry_run):
    restored = 0
    failed = 0
    for path in walk_files(root):
        if not path.endswith(BACKUP_SUFFIX):
            continue
        original = path[:-len(BACKUP_SUFFIX)]
        rel_backup = os.path.relpath(path, root)
        if not os.path.isfile(original):
            out("  ORPHAN    %s (original file missing)" % rel_backup)
            failed += 1
            continue
        rel_original = os.path.relpath(original, root)
        if dry_run:
            out("  WOULD-RESTORE %s" % rel_original)
            restored += 1
            continue
        try:
            with open(path, "rb") as src:
                data = src.read()
            with open(original, "wb") as dst:
                dst.write(data)
            os.remove(path)
            out("  RESTORED  %s" % rel_original)
            restored += 1
        except OSError as exc:
            out("  FAILED    %s: %s" % (rel_original, exc))
            failed += 1
    out("")
    if dry_run:
        out("Revert (dry-run): %d file(s) would be restored, %d problem(s)."
            % (restored, failed))
    else:
        out("Revert: %d file(s) restored, %d failed." % (restored, failed))
    return 0 if failed == 0 else 1


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Apply Mirage native Chromium patches "
                    "(context-anchored templates; see patches/README.md).")
    parser.add_argument(
        "--chromium-dir", required=True,
        help="Path to the Chromium src/ checkout (the directory containing "
             "third_party/, content/, ...).")
    parser.add_argument(
        "--patches-dir", default=None,
        help="Directory with *.patch templates. Default: the patches/ "
             "directory next to this script's parent.")
    parser.add_argument(
        "--dry-run", action="store_true",
        help="Resolve and report everything, write nothing.")
    parser.add_argument(
        "--revert", action="store_true",
        help="Restore every *.mirage-orig backup under --chromium-dir "
             "(patches-dir is ignored in this mode).")
    parser.add_argument(
        "--verbose", action="store_true",
        help="Print MIRAGE-VERIFY notes for every block.")
    args = parser.parse_args(argv)

    force_utf8_stdio()

    root = os.path.abspath(args.chromium_dir)
    if not os.path.isdir(root):
        out("ERROR: --chromium-dir is not a directory: %s" % root)
        return 2

    if args.revert:
        suffix = " (dry-run)" if args.dry_run else ""
        out("Reverting Mirage patches under %s%s" % (root, suffix))
        return do_revert(root, args.dry_run)

    if args.patches_dir:
        patches_dir = os.path.abspath(args.patches_dir)
    else:
        patches_dir = os.path.normpath(os.path.join(
            os.path.dirname(os.path.abspath(__file__)),
            os.pardir, "patches"))
    if not os.path.isdir(patches_dir):
        out("ERROR: --patches-dir is not a directory: %s" % patches_dir)
        return 2

    patch_files = sorted(
        os.path.join(patches_dir, name)
        for name in os.listdir(patches_dir)
        if name.endswith(PATCH_SUFFIX))
    if not patch_files:
        out("WARNING: no %s files found in %s" % (PATCH_SUFFIX, patches_dir))
        return 0

    results = []
    parse_failures = 0

    for patch_path in patch_files:
        try:
            patch = parse_patch_file(patch_path)
        except PatchFormatError as exc:
            out("")
            out("ERROR: %s" % exc)
            parse_failures += 1
            continue

        out("")
        out("[%s] %s" % (patch.patch_id or "?", patch.title))
        out("  target: %s%s" % (
            patch.target, " (dry-run)" if args.dry_run else ""))
        for description in patch.descriptions:
            out("  %s" % description)

        for block in patch.blocks:
            if block.kind == "replace":
                res = process_replace_block(block, root, args.dry_run)
            else:
                res = process_manual_block(block, root)
            results.append(res)
            print_block_result(res, args.verbose)

    counts = {}
    for res in results:
        counts[res["status"]] = counts.get(res["status"], 0) + 1
    errors = counts.get(STATUS_ERROR, 0) + parse_failures

    out("")
    out("=" * 72)
    out("Mirage patch report: %d patch file(s), %d block(s)"
        % (len(patch_files), len(results)))
    for status in (STATUS_APPLIED, STATUS_WOULD_APPLY, STATUS_ALREADY,
                   STATUS_MANUAL, STATUS_ERROR):
        if counts.get(status):
            out("  %-16s %d" % (status, counts[status]))
    if parse_failures:
        out("  %-16s %d" % ("parse-error", parse_failures))
    if errors:
        out("  result: REFUSED - fix the reported anchors and re-run; "
            "nothing ambiguous was applied.")
    else:
        out("  result: OK")
    out("=" * 72)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
