# dmgbuild settings: drag-to-Applications layout written headlessly (no
# Finder/AppleScript, which hangs on GitHub runners). Invoked by
# .github/workflows/build.yml:
#   dmgbuild -s packaging/dmg/settings.py -D app=<path>.app "<volume>" out.dmg
import os.path

app = defines.get("app", "src-tauri/target/release/bundle/macos/AudioDistillery.app")  # noqa: F821

files = [app]
symlinks = {"Applications": "/Applications"}

icon_locations = {
    os.path.basename(app): (140, 130),
    "Applications": (400, 130),
}
background = "builtin-arrow"
window_rect = ((200, 120), (540, 300))
default_view = "icon-view"
icon_size = 100
format = "UDZO"
