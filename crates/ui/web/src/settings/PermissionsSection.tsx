// Settings → Permissions: every site that may, or may not, use the camera,
// the microphone, the location, notifications, pop-ups or the clipboard,
// with each choice to change or take back. The site panel decides for the
// site in front; this is the list of all of them.
import { useCallback, useEffect, useState } from "react";
import { send, type PermissionEntry, type PermissionKind } from "../bridge";
import { Field, Group } from "./fields";
import type { Info } from "./types";

const PERMISSION_LABEL: Record<PermissionKind, string> = {
  camera: "Camera",
  microphone: "Microphone",
  location: "Location",
  notifications: "Notifications",
  popups: "Pop-ups and redirects",
  clipboard: "Clipboard",
};

/** A site's address without `https://`; another scheme stays, to tell it apart. */
const siteName = (origin: string) => origin.replace(/^https:\/\//, "");

export function PermissionsSection({ info }: { info: Info }) {
  // Each set of logins has its own choices: those of the space in front first.
  const logins = info.profiles.filter((p) => !p.ephemeral);
  const inFront = info.spaces.find((s) => s.id === info.active_space)?.profile_id;
  const [profile, setProfile] = useState(() => (logins.find((p) => p.id === inFront) ?? logins.find((p) => p.is_default))?.id);
  const [rows, setRows] = useState<PermissionEntry[] | null>(null);
  const load = useCallback(() => send({ cmd: "all_permissions", profile }).then((r) => setRows(r.rows ?? [])), [profile]);
  useEffect(() => {
    void load();
    // A choice made in a site panel meanwhile shows when the page comes back.
    window.addEventListener("focus", load);
    return () => window.removeEventListener("focus", load);
  }, [load]);
  const set = (row: PermissionEntry, value: PermissionEntry["value"]) =>
    void send({ cmd: "set_profile_permission", profile, origin: row.origin, kind: row.kind, value }).then(load);
  const sites = [...new Set((rows ?? []).map((r) => r.origin))];
  return (
    <>
      {logins.length > 1 && (
        <Group>
          <Field side label="Logins" hint="Each set of logins keeps its own choices.">
            <select className="field" value={profile} onChange={(e) => setProfile(e.target.value)} aria-label="Logins">
              {logins.map((p) => <option key={p.id} value={p.id}>{p.is_default ? "Shared (your main logins)" : p.name}</option>)}
            </select>
          </Field>
        </Group>
      )}
      {rows?.length === 0 && (
        <Group>
          <Field label="No website has a choice of its own" hint="A website that wants the camera, the microphone, your location or notifications asks first. What you answer shows here, to change or take back." />
        </Group>
      )}
      {sites.map((origin) => (
        <Group key={origin} title={siteName(origin)}>
          {rows!.filter((r) => r.origin === origin).map((row) => (
            <Field key={row.kind} side label={PERMISSION_LABEL[row.kind]}>
              <div className="flex items-center gap-2">
                <div className="seg">
                  {(["allow", "block"] as const).map((value) => (
                    <button key={value} data-on={row.value === value || undefined} onClick={() => set(row, value)}>{value === "allow" ? "Allow" : "Block"}</button>
                  ))}
                </div>
                <button className="btn ghost" title={row.kind === "popups" ? "Back to blocked, as for every site" : "The site asks again"} aria-label={`Reset ${PERMISSION_LABEL[row.kind]} for ${siteName(origin)}`} onClick={() => set(row, "ask")}>Reset</button>
              </div>
            </Field>
          ))}
        </Group>
      ))}
    </>
  );
}
