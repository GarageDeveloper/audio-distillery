import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Backdrop } from "./Backdrop";

interface Props {
  onClose: () => void;
}

function Ext({ href, children }: { href: string; children: string }) {
  return (
    <button className="about-link" onClick={() => openUrl(href).catch(() => {})}>
      {children}
    </button>
  );
}

/** About panel: version, license and third-party notices (mirrors README). */
export function AboutDialog({ onClose }: Props) {
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  return (
    <Backdrop onClose={onClose}>
      <div className="modal about-modal">
        <h2>AudioDistillery</h2>
        <div className="subtitle">
          “Still” — split long recordings into tracks, master them live, export
          the album.{version && ` Version ${version}.`}
        </div>

        <div className="about-section">
          <div className="about-title">License</div>
          <p>
            © 2026 Raphaël Enrici — released under the MIT License. Sources and
            license text:{" "}
            <Ext href="https://github.com/GarageDeveloper/audio-distillery">
              github.com/GarageDeveloper/audio-distillery
            </Ext>
          </p>
        </div>

        <div className="about-section">
          <div className="about-title">Third-party notices</div>
          <ul>
            <li>
              VST is a trademark of Steinberg Media Technologies GmbH,
              registered in Europe and other countries. VST 3 hosting uses the
              MIT-licensed <Ext href="https://crates.io/crates/vst3">vst3</Ext>{" "}
              Rust bindings.
            </li>
            <li>Audio Units and macOS are technologies of Apple Inc.</li>
            <li>
              Exports are performed by a bundled{" "}
              <Ext href="https://ffmpeg.org">FFmpeg</Ext> binary (separate
              process), built under the GPL by{" "}
              <Ext href="https://ffmpeg.martin-riedl.de">
                ffmpeg.martin-riedl.de
              </Ext>{" "}
              — sources available there. FFmpeg is a trademark of Fabrice
              Bellard.
            </li>
            <li>
              Built with Tauri, React, Symphonia, lofty, cpal and other
              open-source libraries — thank you all.
            </li>
          </ul>
        </div>

        <div className="modal-foot">
          <button className="btn btn-primary" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </Backdrop>
  );
}
