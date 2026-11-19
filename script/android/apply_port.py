#!/usr/bin/env python3
"""Applies the Android port to a checkout of storytold/pdfcraft.

    python3 script/android/apply_port.py <pdfcraft-checkout>

The new crates and the Gradle project are copied into the checkout first (the workflow does that).
This script then makes the only two edits the port needs to upstream's own files, working from
whatever upstream looks like today, so it keeps working as upstream changes:

  * crates/ui-egui/Cargo.toml: `rfd` and `arboard` keep their current declarations for every target
    except Android, and on Android depend on the stand-ins in crates/craft-rfd-android and
    crates/craft-arboard-android instead.
  * crates/ui-egui/src/lib.rs: on Android, `extern crate` aliases make `rfd::...` and `arboard::...`
    mean the stand-ins, so none of the app's call sites change.

It also lines the port's own crates up with upstream's current eframe version and release version.
It is safe to run twice. Needs `pip install tomlkit`.
"""
import re
import sys
import tomllib
from pathlib import Path

try:
    import tomlkit
except ImportError:
    sys.exit("apply_port: pip install tomlkit")

NOT_ANDROID = 'not(target_os = "android")'
ANDROID = 'cfg(target_os = "android")'
# dependency in the UI crate -> (stand-in crate, its folder under crates/)
SHIMS = {
    "rfd": "craft-rfd-android",
    "arboard": "craft-arboard-android",
}
ALIAS_COMMENT = (
    "// Android: stand-ins (crates/craft-rfd-android, crates/craft-arboard-android) replace these\n"
    "// desktop-only crates, so every `rfd::` and `arboard::` path below resolves to them unchanged.\n"
)


def die(message: str):
    sys.exit(f"apply_port: {message}")


def active_on_android(target_key: str) -> bool:
    """Whether a `[target.'cfg(...)'.dependencies]` table can apply to Android (conservative)."""
    m = re.fullmatch(r"cfg\((.*)\)", target_key.strip())
    if not m:
        return "android" in target_key
    expr = m.group(1)
    # Only the forms upstream uses are understood; anything else is assumed to apply, so it gets
    # narrowed with `not(android)` and the stand-in is added: harmless if it never applied.
    if re.fullmatch(r'target_os = "(linux|macos|windows|ios|freebsd)"', expr) or expr in ("windows", 'target_arch = "wasm32"', 'target_family = "wasm"'):
        return False
    return True


def gate_dependencies(manifest: Path) -> list[str]:
    """Moves each shimmed dependency out of Android's reach and adds its stand-in for Android."""
    doc = tomlkit.parse(manifest.read_text())
    log: list[str] = []
    target = doc.setdefault("target", tomlkit.table(True))

    def deps_table(key: str):
        if key not in target:
            target[key] = tomlkit.table()
        if "dependencies" not in target[key]:
            target[key]["dependencies"] = tomlkit.table()
        return target[key]["dependencies"]

    for dep, shim in SHIMS.items():
        if shim in deps_table(ANDROID):
            continue  # already applied
        sources = []
        if dep in doc.get("dependencies", {}):
            sources.append((None, doc["dependencies"]))
        for key, tbl in doc.get("target", {}).items():
            if dep in tbl.get("dependencies", {}) and active_on_android(key):
                sources.append((key, tbl["dependencies"]))
        if not sources:
            log.append(f"  {dep}: not used by crates/ui-egui any more; no stand-in needed")
            continue
        for key, tbl in sources:
            item = tbl[dep]
            if key is None:
                new_key = f"cfg({NOT_ANDROID})"
            else:
                inner = re.fullmatch(r"cfg\((.*)\)", key.strip())
                if not inner:
                    continue
                new_key = f"cfg(all({inner.group(1)}, {NOT_ANDROID}))"
            del tbl[dep]
            deps_table(new_key)[dep] = item
        inline = tomlkit.inline_table()
        inline["path"] = f"../{shim}"
        deps_table(ANDROID)[shim] = inline
        log.append(f"  {dep}: stays for desktop and web; Android uses {shim}")
    for key in list(target.keys()):  # drop tables the moves emptied
        tbl = target[key]
        if "dependencies" in tbl and not tbl["dependencies"]:
            del tbl["dependencies"]
        if not tbl:
            del target[key]
    manifest.write_text(tomlkit.dumps(doc))
    return log


def alias_in_crate_root(lib: Path) -> list[str]:
    text = lib.read_text()
    wanted = [(shim.replace("-", "_"), dep) for dep, shim in SHIMS.items()]
    missing = [(c, d) for c, d in wanted if f"extern crate {c} as {d};" not in text]
    # only for dependencies the crate actually has (no alias for a stand-in that isn't wired in)
    manifest = tomllib.loads((lib.parent.parent / "Cargo.toml").read_text())
    android_deps = manifest.get("target", {}).get(ANDROID, {}).get("dependencies", {})
    missing = [(c, d) for c, d in missing if c.replace("_", "-") in android_deps]
    if not missing:
        return []
    block = ALIAS_COMMENT + "".join(f'#[cfg(target_os = "android")]\nextern crate {c} as {d};\n' for c, d in missing) + "\n"
    rows = text.split("\n")
    # after the leading `//!` docs and (possibly multi-line) `#![...]` inner attributes
    at = next((i for i, row in enumerate(rows) if re.match(r"(pub(\([a-z]+\))? )?(mod|use|extern|const|static|struct|enum|fn|type|trait|impl|macro_rules!)\b|#\[", row)), len(rows))
    while at > 0 and re.match(r"\s*(///|#\[)", rows[at - 1]):  # keep an item's docs and attributes with it
        at -= 1
    rows[at:at] = block.rstrip("\n").split("\n") + [""]
    lib.write_text("\n".join(rows))
    return [f"  crates/ui-egui/src/lib.rs: aliased {', '.join(c for c, _ in missing)}"]


def align_with_upstream(root: Path, workspace: dict) -> list[str]:
    """Same eframe requirement and version as upstream, so the port's crates share its dependency graph."""
    log = []
    deps = workspace.get("workspace", {}).get("dependencies", {})
    eframe = deps.get("eframe")
    requirement = eframe if isinstance(eframe, str) else (eframe or {}).get("version")
    if not requirement:
        die("upstream's workspace has no eframe version to match")
    version = workspace.get("workspace", {}).get("package", {}).get("version") or workspace.get("package", {}).get("version") or "0.1.0"
    for rel in ("crates/craft-android/Cargo.toml", "apps/pdfcraft-android/Cargo.toml"):
        path = root / rel
        text = path.read_text()
        new = re.sub(r'(eframe = \{ version = ")[^"]+(")', rf"\g<1>{requirement}\g<2>", text)
        new = re.sub(r'(?m)^(version = ")[^"]+(")', rf"\g<1>{version}\g<2>", new, count=1) if rel.startswith("apps/") else new
        if new != text:
            path.write_text(new)
            log.append(f"  {rel}: eframe {requirement}, version {version}")
    return log


def main(root: Path):
    ui = root / "crates/ui-egui"
    for needed in (root / "Cargo.toml", ui / "Cargo.toml", ui / "src/lib.rs", root / "crates/craft-android", root / "apps/pdfcraft-android"):
        if not needed.exists():
            die(f"{needed.relative_to(root)} is missing. Is {root} a PdfCraft checkout with the port files copied in?")
    workspace = tomllib.loads((root / "Cargo.toml").read_text())
    deps = workspace.get("workspace", {}).get("dependencies", {})
    for crate in ("pdfcraft-engine", "pdfcraft-ui-egui"):
        if crate not in deps:
            die(f"upstream no longer has a workspace dependency named {crate}; apps/pdfcraft-android needs updating")
    ui_manifest = tomllib.loads((ui / "Cargo.toml").read_text())
    if ui_manifest["package"]["name"] != "pdfcraft-ui-egui":
        die(f"crates/ui-egui is now called {ui_manifest['package']['name']}; the port expects pdfcraft-ui-egui")

    print("Applying the Android port to", root)
    for line in align_with_upstream(root, workspace) + gate_dependencies(ui / "Cargo.toml") + alias_in_crate_root(ui / "src/lib.rs"):
        print(line)
    print("Done.")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(Path(sys.argv[1]).resolve())
