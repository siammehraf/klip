import { useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ClipItem,
  ClipSourceBadge,
  formatBytes,
  typeLabel
} from "./components/ClipBadge";

interface Phase1Status {
  version: string;
  paused: boolean;
  hotkey: string;
  hotkey_registered: boolean;
}

function formatTime(ts: number): string {
  return new Date(ts).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit"
  });
}

export default function App() {
  const [paused, setPaused] = useState(false);
  const [version, setVersion] = useState("0.1.0");
  const [hotkey, setHotkey] = useState("Ctrl+Shift+Backquote");
  const [hotkeyOk, setHotkeyOk] = useState(true);
  const [excludedCount, setExcludedCount] = useState(0);
  const [clips, setClips] = useState<ClipItem[]>([]);
  const [search, setSearch] = useState("");
  const [pinnedOnly, setPinnedOnly] = useState(false);
  const [copiedId, setCopiedId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadClips = async (query: string, pinned: boolean) => {
    try {
      const list = await invoke<ClipItem[]>("list_clips", {
        search: query,
        pinnedOnly: pinned,
        limit: 200
      });
      setClips(list);
    } catch (err) {
      setError(String(err));
    }
  };

  useEffect(() => {
    Promise.all([
      invoke<Phase1Status>("get_phase1_status"),
      invoke<string[]>("get_excluded_apps"),
      invoke<ClipItem[]>("list_clips", { search: "", pinnedOnly: false, limit: 200 })
    ])
      .then(([status, excluded, initialClips]) => {
        setPaused(status.paused);
        setVersion(status.version);
        setHotkey(status.hotkey);
        setHotkeyOk(status.hotkey_registered);
        setExcludedCount(excluded.length);
        setClips(initialClips);
      })
      .catch((err) => setError(String(err)));

    const unlistenPause = listen<boolean>("klip://paused-changed", (event) => {
      setPaused(event.payload);
    });

    const unlistenHotkey = listen<boolean>("klip://hotkey-status", (event) => {
      setHotkeyOk(event.payload);
    });

    const unlistenAdded = listen<ClipItem>("klip://clip-added", (event) => {
      const added = event.payload;
      setClips((prev) => {
        if (prev.some((c) => c.id === added.id)) return prev;
        return [added, ...prev].slice(0, 200);
      });
    });

    const unlistenDeleted = listen<number>("klip://clip-deleted", (event) => {
      setClips((prev) => prev.filter((c) => c.id !== event.payload));
    });

    const unlistenPinned = listen<[number, boolean]>("klip://clip-pinned", (event) => {
      const [id, isPinned] = event.payload;
      setClips((prev) =>
        prev
          .map((c) => (c.id === id ? { ...c, is_pinned: isPinned } : c))
          .filter((c) => (!pinnedOnly ? true : c.is_pinned))
      );
    });

    const unlistenPruned = listen<number[]>("klip://clips-pruned", (event) => {
      const removedSet = new Set(event.payload);
      setClips((prev) => prev.filter((c) => !removedSet.has(c.id)));
    });

    return () => {
      unlistenPause.then((fn) => fn());
      unlistenHotkey.then((fn) => fn());
      unlistenAdded.then((fn) => fn());
      unlistenDeleted.then((fn) => fn());
      unlistenPinned.then((fn) => fn());
      unlistenPruned.then((fn) => fn());
    };
  }, [pinnedOnly]);

  useEffect(() => {
    loadClips(search, pinnedOnly);
  }, [search, pinnedOnly]);

  const handleTogglePause = async () => {
    try {
      const next = await invoke<boolean>("toggle_pause");
      setPaused(next);
    } catch (err) {
      setError(String(err));
    }
  };

  const handleOpenPopup = async () => {
    await invoke("open_quick_popup");
  };

  const handleCopy = async (clipId: number) => {
    try {
      await invoke("copy_clip", { clipId, plainOnly: false });
      setCopiedId(clipId);
      setTimeout(() => {
        setCopiedId((curr) => (curr === clipId ? null : curr));
      }, 1200);
    } catch (err) {
      setError(String(err));
    }
  };

  const handleTogglePin = async (clipId: number) => {
    try {
      const nextPinned = await invoke<boolean>("toggle_pin_clip", { clipId });
      setClips((prev) =>
        prev
          .map((c) => (c.id === clipId ? { ...c, is_pinned: nextPinned } : c))
          .filter((c) => (!pinnedOnly ? true : c.is_pinned))
      );
    } catch (err) {
      setError(String(err));
    }
  };

  const handleDelete = async (clipId: number) => {
    try {
      await invoke("delete_clip", { clipId });
      setClips((prev) => prev.filter((c) => c.id !== clipId));
    } catch (err) {
      setError(String(err));
    }
  };

  const displayHotkey = hotkey.replace("Backquote", "~");

  return (
    <div className="flex h-full flex-col bg-[#0e1015] text-zinc-100">
      <header className="flex flex-wrap items-center justify-between gap-3 border-b border-zinc-800/80 px-5 py-3">
        <div className="flex items-center gap-2.5">
          <div className="flex h-7 w-7 items-center justify-center rounded-md border border-zinc-700/70 bg-zinc-900">
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden="true">
              <path
                d="M7 4v16M7 12l8-7M10.5 9.5L17 20"
                stroke="#f4f4f5"
                strokeWidth="2.2"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
          </div>
          <h1 className="text-sm font-semibold tracking-tight">
            Klip <span className="font-normal text-zinc-500">v{version}</span>
          </h1>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <span
            className={`rounded-md border px-2.5 py-1 text-[11px] ${
              hotkeyOk
                ? "border-zinc-800 bg-zinc-900/80 text-zinc-300"
                : "border-red-800/70 bg-red-950/40 text-red-300"
            }`}
          >
            Hotkey {displayHotkey}: {hotkeyOk ? "Active" : "Conflict (in use by another app)"}
          </span>
          <button
            type="button"
            onClick={handleOpenPopup}
            className="rounded-md border border-zinc-700 bg-zinc-900 px-2.5 py-1 text-xs text-zinc-200 hover:bg-zinc-800"
          >
            Quick Popup
          </button>
          <span className="rounded-md border border-zinc-800 bg-zinc-900/80 px-2.5 py-1 text-[11px] text-zinc-400">
            {excludedCount} excluded
          </span>
          <button
            type="button"
            onClick={handleTogglePause}
            className={`rounded-md border px-3 py-1 text-xs font-medium ${
              paused
                ? "border-amber-700/60 bg-amber-950/40 text-amber-200 hover:bg-amber-900/40"
                : "border-emerald-700/60 bg-emerald-950/40 text-emerald-200 hover:bg-emerald-900/40"
            }`}
          >
            {paused ? "Paused \u2014 Resume" : "Listening \u2014 Pause"}
          </button>
        </div>
      </header>

      <div className="flex items-center justify-between gap-3 border-b border-zinc-800/70 bg-[#12151d] px-5 py-2.5">
        <div className="flex flex-1 items-center gap-2">
          <input
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search all words/substrings (English, Bangla, app, or site domain)..."
            className="w-full max-w-lg rounded-md border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-xs text-zinc-100 placeholder:text-zinc-500 outline-none focus:border-zinc-500"
          />
          <button
            type="button"
            onClick={() => setPinnedOnly((p) => !p)}
            className={`rounded-md border px-2.5 py-1.5 text-xs font-medium ${
              pinnedOnly
                ? "border-sky-600 bg-sky-950/60 text-sky-200"
                : "border-zinc-700 bg-zinc-900 text-zinc-300"
            }`}
          >
            {pinnedOnly ? "Pinned" : "All"}
          </button>
        </div>
        <span className="text-xs text-zinc-400">{clips.length} clips</span>
      </div>

      <main className="flex-1 overflow-auto p-5">
        {error && (
          <div className="mb-4 rounded-lg border border-red-700/60 bg-red-950/30 p-3 text-xs text-red-400">
            {error}
          </div>
        )}

        {clips.length === 0 ? (
          <div className="mx-auto mt-12 max-w-md rounded-xl border border-zinc-800 bg-[#14171f] p-6 text-center">
            <p className="text-sm font-medium">No matching clips</p>
            <p className="mt-1.5 text-xs text-zinc-400">
              Press {displayHotkey} anywhere to open the Quick Popup, or copy text/images to see them captured here.
            </p>
          </div>
        ) : (
          <div className="mx-auto max-w-4xl space-y-2.5">
            {clips.map((clip) => (
              <article
                key={clip.id}
                className="rounded-xl border border-zinc-800/90 bg-[#14171f] p-3.5"
              >
                <div className="flex items-center justify-between gap-3">
                  <div className="flex flex-wrap items-center gap-2 text-xs">
                    <ClipSourceBadge clip={clip} isDark={true} />
                    <span className="rounded border border-zinc-700 bg-zinc-900 px-1.5 py-0.5 text-[10px] font-medium text-zinc-300">
                      {typeLabel(clip.clip_type)}
                    </span>
                    {clip.clip_type === "color" && clip.text_content && (
                      <span
                        className="inline-block h-3.5 w-3.5 rounded-sm border border-zinc-500/50"
                        style={{ backgroundColor: clip.text_content.trim() }}
                      />
                    )}
                    <span className="text-[11px] text-zinc-500">
                      {formatTime(clip.created_at)} {"\u2022"} {formatBytes(clip.content_size)}
                    </span>
                  </div>

                  <div className="flex items-center gap-1.5">
                    <button
                      type="button"
                      onClick={() => handleCopy(clip.id)}
                      className="rounded border border-zinc-700 bg-zinc-900 px-2 py-0.5 text-[11px] text-zinc-200 hover:bg-zinc-800"
                    >
                      {copiedId === clip.id ? "Copied" : "Copy"}
                    </button>
                    <button
                      type="button"
                      onClick={() => handleTogglePin(clip.id)}
                      className={`rounded border px-2 py-0.5 text-[11px] ${
                        clip.is_pinned
                          ? "border-amber-600/70 bg-amber-950/50 text-amber-200"
                          : "border-zinc-700 bg-zinc-900 text-zinc-300 hover:bg-zinc-800"
                      }`}
                    >
                      {clip.is_pinned ? "Pinned" : "Pin"}
                    </button>
                    <button
                      type="button"
                      onClick={() => handleDelete(clip.id)}
                      className="rounded border border-zinc-700 bg-zinc-900 px-2 py-0.5 text-[11px] text-zinc-400 hover:border-red-800 hover:text-red-300"
                    >
                      Delete
                    </button>
                  </div>
                </div>

                {clip.clip_type === "image" && clip.image_path ? (
                  <div className="mt-2.5 flex items-start gap-3">
                    <img
                      src={convertFileSrc(clip.image_path)}
                      alt="Captured clip"
                      className="max-h-40 max-w-full rounded-lg border border-zinc-800 bg-zinc-950 object-contain"
                    />
                    <span className="font-mono text-[11px] text-zinc-400">
                      {clip.text_content}
                    </span>
                  </div>
                ) : (
                  <pre
                    className={`mt-2 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-lg border border-zinc-800/80 bg-zinc-900/70 p-2.5 text-xs text-zinc-200 ${
                      clip.clip_type === "code" ? "font-mono" : ""
                    }`}
                  >
                    {clip.text_content ?? "(Rich HTML content)"}
                  </pre>
                )}
              </article>
            ))}
          </div>
        )}
      </main>
    </div>
  );
}