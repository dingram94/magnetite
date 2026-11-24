// Settings → Bookmarks: the bookmarks of one space (how many, its pinned
// tabs added to them, deleting them all), and the bookmarks as a file, to
// import and to export.
import { useState } from "react";
import { send } from "../bridge";
import { useArmed } from "../dismiss";
import { Field, Group } from "./fields";
import { ExportBookmarks, ImportBookmarksFile } from "./ImportBrowsers";
import { PickList } from "./PickList";
import { spaceOptions } from "./SpacesSection";
import type { Info } from "./types";

type Space = Info["spaces"][number];

/** All that is about the bookmarks of one space. */
function SpaceBookmarks({ space, reload }: { space: Space; reload: () => Promise<unknown> }) {
  const { armed, arm } = useArmed(4000);
  const [said, setSaid] = useState<string>();
  const say = (text: string) => { setSaid(text); void reload(); };
  const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;
  const toBookmarks = (remove: boolean) =>
    send({ cmd: "pinned_to_bookmarks", space: space.id, remove }).then(
      (r) => say(`${r.bookmarks ? `${plural(r.bookmarks, "bookmark")} added` : "Every pinned tab is a bookmark already"}${remove ? `; ${plural(r.pinned, "pinned tab")} closed.` : "."}`),
      (e: unknown) => setSaid(e instanceof Error ? e.message : String(e)),
    );
  return (
    <Group title={`Bookmarks in ${space.title}`} hint="Saved addresses of this space, apart from its tabs.">
      <Field side label={space.bookmarks === 0 ? "No bookmarks" : plural(space.bookmarks, "bookmark")} hint="Deleting removes all of them, folders included. It cannot be undone.">
        <button className="btn" disabled={!space.bookmark_tree} data-danger={armed === "clear" || undefined} onClick={() => arm("clear", () => void send({ cmd: "clear_bookmarks", space: space.id }).then((r) => say(`${plural(r.bookmarks, "bookmark")} deleted.`)))}>
          {armed === "clear" ? "Click again to delete" : "Delete bookmarks…"}
        </button>
      </Field>
      <Field side label="Add pinned tabs to bookmarks" hint={`${space.pinned === 0 ? "This space has no pinned tabs." : `${plural(space.pinned, "pinned tab")}.`} Copy keeps them pinned. Move closes them after: they are bookmarks then, and in the archive. Folders come along; smart folders do not.`}>
        <div className="flex gap-2">
          <button className="btn" disabled={space.pinned === 0} onClick={() => void toBookmarks(false)}>Copy</button>
          <button className="btn" disabled={space.pinned === 0} data-danger={armed === "move" || undefined} onClick={() => arm("move", () => void toBookmarks(true))}>
            {armed === "move" ? "Click again to move" : "Move…"}
          </button>
        </div>
      </Field>
      {said && <p className="set-row set-hint" role="status" style={{ marginTop: 0 }}>{said}</p>}
    </Group>
  );
}

/** `item` names the space to start with; without one, or once it is gone, the space in front. */
export function BookmarksSection({ info, reload, item }: { info: Info; reload: () => Promise<unknown>; item?: string }) {
  // The space whose bookmarks show, until another is chosen.
  const [chosen, setChosen] = useState(item);
  const space = info.spaces.find((s) => s.id === chosen) ?? info.spaces.find((s) => s.id === info.active_space) ?? info.spaces[0];
  return (
    <>
      {info.spaces.length > 1 && (
        <Group>
          <Field side label="Space" hint="Every space has bookmarks of its own. They show in the sidebar's bookmarks view of that space.">
            <PickList options={spaceOptions(info.spaces)} chosen={space ? [space.id] : []} label="Space whose bookmarks show" width={220} align="right" onChoose={([to]) => setChosen(to)} />
          </Field>
        </Group>
      )}
      {space && <SpaceBookmarks key={space.id} space={space} reload={reload} />}
      <Group title="As a file">
        <ImportBookmarksFile key={space?.id} spaces={info.spaces} current={space?.id ?? info.active_space} imported={reload} />
        <ExportBookmarks spaces={info.spaces} />
      </Group>
    </>
  );
}
