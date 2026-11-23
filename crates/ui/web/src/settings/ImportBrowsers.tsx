// The browsers on this computer to import from, each at a click (Settings → Data and privacy), and the bookmarks as a file, to import and export (Settings → Bookmarks).
import { useEffect, useRef, useState } from "react";
import { type BookmarkFormat, type ImportSource, send } from "../bridge";
import { Field } from "./fields";
import { PickList } from "./PickList";
import { spaceOptions } from "./SpacesSection";
import type { Info } from "./types";

/** Browsers on this computer to import from; every import adds spaces. */
export function ImportBrowsers() {
  const [sources, setSources] = useState<ImportSource[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [result, setResult] = useState<{ text: string; failed: boolean } | null>(null);
  useEffect(() => {
    send({ cmd: "import_sources" }).then((r) => setSources(r.sources ?? []));
  }, []);
  const run = (source: ImportSource) => {
    setBusy(source.key);
    setResult(null);
    send({ cmd: "import_from", key: source.key })
      .then((r) => setResult({ text: `${source.name}: added ${count(r.spaces, "space")} with ${count(r.tabs, "tab")}${r.bookmarks ? ` and ${count(r.bookmarks, "bookmark")}` : ""}.`, failed: false }))
      .catch((e: unknown) => setResult({ text: `${source.name}: ${e instanceof Error ? e.message : String(e)}`, failed: true }))
      .finally(() => setBusy(null));
  };
  return (
    <Field
      label="Import from another browser"
      hint="Adds a space for each space or workspace of that browser, or for each profile that has neither, with its tabs, tab groups and bookmarks. Your sidebar stays as it is. Passwords, cookies and extensions are not copied, and nothing leaves this computer."
    >
      {sources?.length === 0 && <p className="set-hint">No other browser found on this computer.</p>}
      {sources && sources.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {sources.map((s) => (
            <button key={s.key} className="btn" disabled={busy !== null} onClick={() => run(s)}>
              {busy === s.key ? `Importing ${s.name}…` : s.name}
            </button>
          ))}
        </div>
      )}
      {result && <p className="set-hint" style={result.failed ? { color: "var(--danger)" } : undefined}>{result.text}</p>}
    </Field>
  );
}

/** A bookmarks file, as Safari and other browsers export it: its bookmarks join those of the space chosen (`current` at first). `imported` hears when some were added. */
type Space = Info["spaces"][number];

export function ImportBookmarksFile({ spaces, current, imported }: { spaces: Space[]; current: string; imported?: () => Promise<unknown> }) {
  const input = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ text: string; failed: boolean } | null>(null);
  // The space the bookmarks go to: the one in front, until another is chosen.
  const [chosen, setChosen] = useState<string | null>(null);
  const space = spaces.find((s) => s.id === chosen) ?? spaces.find((s) => s.id === current) ?? spaces[0];
  const read = async (file: File) => {
    setBusy(true);
    setResult(null);
    try {
      const r = await send({ cmd: "import_bookmarks", html: await file.text(), space: space?.id });
      const where = space ? space.title : "the current space";
      setResult({ text: r.bookmarks ? `Added ${count(r.bookmarks, "bookmark")} to ${where}.` : `Every bookmark of that file is in ${where} already.`, failed: false });
      // The page counts the bookmarks of its space: it reads them again.
      void imported?.().catch(() => undefined);
    } catch (e: unknown) {
      setResult({ text: e instanceof Error ? e.message : String(e), failed: true });
    } finally {
      setBusy(false);
    }
  };
  return (
    <Field
      label="Import a bookmarks file"
      hint="The file a browser writes with “Export Bookmarks” (Safari: File → Export → Bookmarks…), or a JSON or CSV file exported below. Its bookmarks join those of the space chosen; addresses already there are skipped."
    >
      <input
        ref={input}
        type="file"
        accept=".html,.htm,.json,.csv,text/html,application/json,text/csv"
        hidden
        onChange={(e) => {
          const file = e.target.files?.[0];
          // The same file can be chosen again.
          e.target.value = "";
          if (file) void read(file);
        }}
      />
      <div className="flex flex-wrap items-center gap-2">
        {spaces.length > 1 && (
          <>
            <span className="text-[13px] text-[var(--muted)]">Into</span>
            <PickList options={spaceOptions(spaces)} chosen={space ? [space.id] : []} label="Space the bookmarks go to" width={200} onChoose={([to]) => to && setChosen(to)} />
          </>
        )}
        <button className="btn" disabled={busy} onClick={() => input.current?.click()}>{busy ? "Importing…" : "Choose file…"}</button>
      </div>
      {result && <p className="set-hint" role="status" style={result.failed ? { color: "var(--danger)" } : undefined}>{result.text}</p>}
    </Field>
  );
}

const FORMATS: readonly { format: BookmarkFormat; label: string; type: string }[] = [
  { format: "html", label: "HTML, for another browser", type: "text/html" },
  { format: "json", label: "JSON", type: "application/json" },
  { format: "csv", label: "CSV, for a spreadsheet", type: "text/csv" },
];

/**
 * The bookmarks of the spaces chosen or of all, written to a file the browser
 * downloads like any other: it lands where downloads go.
 */
export function ExportBookmarks({ spaces }: { spaces: Space[] }) {
  // The spaces to export; none chosen is all of them.
  const [chosen, setChosen] = useState<string[]>([]);
  const from = chosen.filter((id) => spaces.some((s) => s.id === id));
  const [format, setFormat] = useState<BookmarkFormat>("html");
  const [result, setResult] = useState<{ text: string; failed: boolean } | null>(null);
  const run = async () => {
    setResult(null);
    try {
      const r = await send({ cmd: "export_bookmarks", format, spaces: from });
      const kind = FORMATS.find((f) => f.format === format)!;
      const url = URL.createObjectURL(new Blob([r.text], { type: `${kind.type};charset=utf-8` }));
      const link = document.createElement("a");
      link.href = url;
      link.download = r.name;
      link.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 10_000);
      setResult({ text: `${count(r.bookmarks, "bookmark")} written to ${r.name}, where your downloads go.`, failed: false });
    } catch (e: unknown) {
      setResult({ text: e instanceof Error ? e.message : String(e), failed: true });
    }
  };
  return (
    <Field label="Export bookmarks" hint="The bookmarks of all your spaces, or of the ones you tick, as a file: HTML is what every browser imports, JSON and CSV are for your own use. Tabs and favorites are not bookmarks and are not in it.">
      <div className="flex flex-wrap items-center gap-2">
        {spaces.length > 1 && (
          <PickList several none="All spaces" options={spaceOptions(spaces)} chosen={from} label="Spaces to export" width={200} onChoose={setChosen} />
        )}
        <select className="field" style={{ width: 220 }} value={format} aria-label="Format" onChange={(e) => setFormat(e.target.value as BookmarkFormat)}>
          {FORMATS.map((f) => <option key={f.format} value={f.format}>{f.label}</option>)}
        </select>
        <button className="btn" onClick={() => void run()}>Export…</button>
      </div>
      {result && <p className="set-hint" role="status" style={result.failed ? { color: "var(--danger)" } : undefined}>{result.text}</p>}
    </Field>
  );
}

const count = (n: number, noun: string) => `${n} ${noun}${n === 1 ? "" : "s"}`;
