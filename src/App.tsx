import { useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface SettingEntry {
  key: string;
  value: string;
}

interface Phase1Status {
  version: string;
  db_path: string;
  images_dir: string;
  tables: string[];
  paused: boolean;
  theme: "dark" | "light";
  font_family: string;
  settings: SettingEntry[];
}

interface ClipItem {
  id: number;
  clip_type: string;
  text_content: string | null;
  html_content: string | null;
  image_path: string | null;
  content_size: number;
  source_app: string | null;
  source_icon_path: string | null;
  is_pinned: boolean;
  created_at: number;
}

const SYSTEM_DEFAULT_FONT = "Segoe UI, Nirmala UI, sans-serif";

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

function formatTime(ts: number): string {
  return new Date(ts).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit"
  });
}

function typeLabel(clipType: string): string {
  switch (clipType) {
    case "rich_text":
      return "Rich Text";
    case "image":
      return "Image";
    case "link":
      return "Link";
    case "email":
      return "Email";
    case "color":
      return "Color";
    case "code":
      return "Code";
    default:
      return "Text";
  }
}

export default function App() {
  const [status, setStatus] = useState<Phase1Status | null>(null);
  const [clips, setClips] = useState<ClipItem[]>([]);
  const [search, setSearch] = useState("");
  const [pinnedOnly, setPinnedOnly] = useState(false);
  const [fonts, setFonts] = useState<string[]>([]);
  const [excludedApps, setExcludedApps] = useState<string[]>([]);
  const [newExcludedApp, setNewExcludedApp] = useState("");
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

  const loadAll = async () => {
    try {
      const [data, installedFonts, excluded, initialClips] = await Promise.all([
        invoke<Phase1Status>("get_phase1_status"),
        invoke<string[]>("list_installed_fonts"),
        invoke<string[]>("get_excluded_apps"),
        invoke<ClipItem[]>("list_clips", { search: "", pinnedOnly: false, limit: 200 })
      ]);
      setStatus(data);
      setFonts(installedFonts);
      setExcludedApps(excluded);
      setClips(initialClips);
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  };

  useEffect(() => {
    loadAll();

    const unlistenPause = listen<boolean>("klip://paused-changed", (event) => {
      setStatus((prev) => (prev ? { ...prev, paused: event.payload } : prev));
    });

    const unlistenTheme = listen<"dark" | "light">("klip://theme-changed", (event) => {
      setStatus((prev) => (prev ? { ...prev, theme: event.payload } : prev));
    });

    const unlistenAdded = listen<ClipItem>("klip://clip-added", (event) => {
      const added = event.payload;
      setClips((prev) => {
        if (prev.some((c) => c.id === added.id)) return prev;
        return [added, ...prev].slice(0, 200);
      });
    });

    const unlistenPruned = listen<number[]>("klip://clips-pruned", (event) => {
      const removedSet = new Set(event.payload);
      setClips((prev) => prev.filter((c) => !removedSet.has(c.id)));
    });

    return () => {
      unlistenPause.then((fn) => fn());
      unlistenTheme.then((fn) => fn());
      unlistenAdded.then((fn) => fn());
      unlistenPruned.then((fn) => fn());
    };
  }, []);

  useEffect(() => {
    loadClips(search, pinnedOnly);
  }, [search, pinnedOnly]);

  const handleTogglePause = async () => {
    try {
      const paused = await invoke<boolean>("toggle_pause");
      setStatus((prev) => (prev ? { ...prev, paused } : prev));
    } catch (err) {
      setError(String(err));
    }
  };

  const handleToggleTheme = async () => {
    if (!status) return;
    const nextTheme = status.theme === "dark" ? "light" : "dark";
    try {
      await invoke("set_theme", { theme: nextTheme });
      setStatus((prev) => (prev ? { ...prev, theme: nextTheme } : prev));
    } catch (err) {
      setError(String(err));
    }
  };

  const handleFontChange = async (nextFont: string) => {
    try {
      await invoke("set_font_family", { fontFamily: nextFont });
      setStatus((prev) => (prev ? { ...prev, font_family: nextFont } : prev));
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

  const handleAddExcludedApp = async () => {
    const val = newExcludedApp.trim();
    if (!val) return;
    if (excludedApps.some((a) => a.toLowerCase() === val.toLowerCase())) {
      setNewExcludedApp("");
      return;
    }
    try {
      const updated = await invoke<string[]>("set_excluded_apps", {
        apps: [...excludedApps, val]
      });
      setExcludedApps(updated);
      setNewExcludedApp("");
    } catch (err) {
      setError(String(err));
    }
  };

  const handleRemoveExcludedApp = async (target: string) => {
    try {
      const updated = await invoke<string[]>("set_excluded_apps", {
        apps: excludedApps.filter((a) => a !== target)
      });
      setExcludedApps(updated);
    } catch (err) {
      setError(String(err));
    }
  };

  const isDark = (status?.theme ?? "dark") === "dark";
  const activeFont = status?.font_family || SYSTEM_DEFAULT_FONT;

  return (
    <div
      style={{ fontFamily: activeFont }}
      className={`flex h-full flex-col ${
        isDark ? "bg-[#0e1015] text-zinc-100" : "bg-zinc-50 text-zinc-900"
      }`}
    >
      {/* Header */}
      <header
        className={`flex flex-wrap items-center justify-between gap-3 border-b px-5 py-3.5 ${
          isDark ? "border-zinc-800/80" : "border-zinc-200 bg-white"
        }`}
      >
        <div className="flex items-center gap-3">
          <div
            className={`flex h-8 w-8 items-center justify-center rounded-lg border ${
              isDark ? "border-zinc-700/70 bg-zinc-900" : "border-zinc-300 bg-zinc-100"
            }`}
          >
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" aria-hidden="true">
              <path
                d="M7 4v16M7 12l8-7M10.5 9.5L17 20"
                stroke={isDark ? "#f4f4f5" : "#18181b"}
                strokeWidth="2.2"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
          </div>
          <div>
            <h1 className="text-sm font-semibold tracking-tight">
              Klip <span className="font-normal opacity-60">v{status?.version ?? "0.1.0"}</span>
            </h1>
            <p className={`text-[11px] ${isDark ? "text-zinc-400" : "text-zinc-500"}`}>
              Phase 2 \u2014 Native Windows Clipboard Listener &amp; Capture Engine
            </p>
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <select
            value={activeFont}
            onChange={(e) => handleFontChange(e.target.value)}
            aria-label="Font family"
            className={`max-w-[180px] rounded-md border px-2 py-1 text-xs ${
              isDark
                ? "border-zinc-700 bg-zinc-900 text-zinc-200"
                : "border-zinc-300 bg-zinc-50 text-zinc-900"
            }`}
          >
            <option value={SYSTEM_DEFAULT_FONT}>System Default Font</option>
            {fonts.map((fontName) => (
              <option key={fontName} value={`"${fontName}", "Nirmala UI", sans-serif`}>
                {fontName}
              </option>
            ))}
          </select>

          <button
            type="button"
            onClick={handleToggleTheme}
            className={`rounded-md border px-2.5 py-1 text-xs font-medium ${
              isDark
                ? "border-zinc-700 bg-zinc-900 text-zinc-200 hover:bg-zinc-800"
                : "border-zinc-300 bg-zinc-100 text-zinc-800 hover:bg-zinc-200"
            }`}
          >
            {isDark ? "Dark" : "Light"}
          </button>

          <button
            type="button"
            onClick={handleTogglePause}
            className={`rounded-md border px-2.5 py-1 text-xs font-medium ${
              status?.paused
                ? isDark
                  ? "border-amber-700/60 bg-amber-950/40 text-amber-200"
                  : "border-amber-400 bg-amber-50 text-amber-800"
                : isDark
                ? "border-emerald-700/60 bg-emerald-950/40 text-emerald-200"
                : "border-emerald-400 bg-emerald-50 text-emerald-800"
            }`}
          >
            {status?.paused ? "Paused" : "Listening"}
          </button>
        </div>
      </header>

      {/* Search + Excluded Apps Bar */}
      <div
        className={`flex flex-wrap items-center justify-between gap-3 border-b px-5 py-2.5 ${
          isDark ? "border-zinc-800/70 bg-[#12151d]" : "border-zinc-200 bg-zinc-100/60"
        }`}
      >
        <div className="flex flex-1 items-center gap-2 min-w-[240px]">
          <input
            type="text"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Exact search (English / Bangla / app name)..."
            className={`w-full max-w-sm rounded-md border px-3 py-1.5 text-xs outline-none ${
              isDark
                ? "border-zinc-700 bg-zinc-900 text-zinc-100 placeholder:text-zinc-500 focus:border-zinc-500"
                : "border-zinc-300 bg-white text-zinc-900 placeholder:text-zinc-400 focus:border-zinc-500"
            }`}
          />
          <button
            type="button"
            onClick={() => setPinnedOnly((p) => !p)}
            className={`rounded-md border px-2.5 py-1.5 text-xs font-medium ${
              pinnedOnly
                ? isDark
                  ? "border-sky-600 bg-sky-950/60 text-sky-200"
                  : "border-sky-500 bg-sky-50 text-sky-800"
                : isDark
                ? "border-zinc-700 bg-zinc-900 text-zinc-300"
                : "border-zinc-300 bg-white text-zinc-700"
            }`}
          >
            {pinnedOnly ? "Pinned Only" : "All History"}
          </button>
          <span className={`text-xs ${isDark ? "text-zinc-400" : "text-zinc-500"}`}>
            {clips.length} clips
          </span>
        </div>

        <div className="flex flex-wrap items-center gap-1.5">
          <input
            type="text"
            value={newExcludedApp}
            onChange={(e) => setNewExcludedApp(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") handleAddExcludedApp();
            }}
            placeholder="Exclude app (e.g. notepad.exe)"
            className={`w-44 rounded-md border px-2.5 py-1 text-xs outline-none ${
              isDark
                ? "border-zinc-700 bg-zinc-900 text-zinc-100 placeholder:text-zinc-500"
                : "border-zinc-300 bg-white text-zinc-900 placeholder:text-zinc-400"
            }`}
          />
          <button
            type="button"
            onClick={handleAddExcludedApp}
            className={`rounded-md border px-2.5 py-1 text-xs ${
              isDark
                ? "border-zinc-700 bg-zinc-800 text-zinc-200 hover:bg-zinc-700"
                : "border-zinc-300 bg-white text-zinc-800 hover:bg-zinc-100"
            }`}
          >
            Exclude
          </button>
          {excludedApps.map((appName) => (
            <button
              key={appName}
              type="button"
              onClick={() => handleRemoveExcludedApp(appName)}
              title="Click to remove exclusion"
              className={`rounded border px-2 py-0.5 text-[11px] font-mono ${
                isDark
                  ? "border-rose-800/70 bg-rose-950/40 text-rose-200 hover:bg-rose-900/50"
                  : "border-rose-300 bg-rose-50 text-rose-700 hover:bg-rose-100"
              }`}
            >
              {appName} &times;
            </button>
          ))}
        </div>
      </div>

      {/* Live Clipboard Feed */}
      <main className="flex-1 overflow-auto p-5">
        {error && (
          <div className="mb-4 rounded-lg border border-red-700/60 bg-red-950/30 p-3 text-xs text-red-400">
            {error}
          </div>
        )}

        {clips.length === 0 ? (
          <div
            className={`mx-auto mt-12 max-w-md rounded-xl border p-6 text-center ${
              isDark ? "border-zinc-800 bg-[#14171f]" : "border-zinc-200 bg-white"
            }`}
          >
            <p className="text-sm font-medium">No captured clips yet</p>
            <p className={`mt-1.5 text-xs ${isDark ? "text-zinc-400" : "text-zinc-500"}`}>
              Copy text (English or Bangla), a link, hex color (#3b82f6), code snippet, or a screenshot (Win+Shift+S). Clips appear here instantly.
            </p>
          </div>
        ) : (
          <div className="mx-auto max-w-4xl space-y-2.5">
            {clips.map((clip) => (
              <article
                key={clip.id}
                className={`rounded-xl border p-3.5 ${
                  isDark ? "border-zinc-800/90 bg-[#14171f]" : "border-zinc-200 bg-white"
                }`}
              >
                <div className="flex items-center justify-between gap-3">
                  <div className="flex items-center gap-2 text-xs">
                    {clip.source_icon_path ? (
                      <img
                        src={convertFileSrc(clip.source_icon_path)}
                        alt=""
                        className="h-4 w-4 rounded-sm object-contain"
                      />
                    ) : (
                      <span
                        className={`inline-block h-4 w-4 rounded-sm ${
                          isDark ? "bg-zinc-800" : "bg-zinc-200"
                        }`}
                      />
                    )}
                    <span className="font-medium">
                      {clip.source_app ?? "Unknown app"}
                    </span>
                    <span
                      className={`rounded border px-1.5 py-0.5 text-[10px] font-medium ${
                        isDark
                          ? "border-zinc-700 bg-zinc-900 text-zinc-300"
                          : "border-zinc-300 bg-zinc-100 text-zinc-700"
                      }`}
                    >
                      {typeLabel(clip.clip_type)}
                    </span>
                    {clip.clip_type === "color" && clip.text_content && (
                      <span
                        className="inline-block h-3.5 w-3.5 rounded-sm border border-zinc-500/50"
                        style={{ backgroundColor: clip.text_content.trim() }}
                      />
                    )}
                    <span className={`text-[11px] ${isDark ? "text-zinc-500" : "text-zinc-400"}`}>
                      {formatTime(clip.created_at)} \u2022 {formatBytes(clip.content_size)}
                    </span>
                  </div>

                  <div className="flex items-center gap-1.5">
                    <button
                      type="button"
                      onClick={() => handleTogglePin(clip.id)}
                      className={`rounded border px-2 py-0.5 text-[11px] ${
                        clip.is_pinned
                          ? isDark
                            ? "border-amber-600/70 bg-amber-950/50 text-amber-200"
                            : "border-amber-400 bg-amber-50 text-amber-800"
                          : isDark
                          ? "border-zinc-700 bg-zinc-900 text-zinc-300 hover:bg-zinc-800"
                          : "border-zinc-300 bg-zinc-50 text-zinc-700 hover:bg-zinc-100"
                      }`}
                    >
                      {clip.is_pinned ? "Pinned" : "Pin"}
                    </button>
                    <button
                      type="button"
                      onClick={() => handleDelete(clip.id)}
                      className={`rounded border px-2 py-0.5 text-[11px] ${
                        isDark
                          ? "border-zinc-700 bg-zinc-900 text-zinc-400 hover:border-red-800 hover:text-red-300"
                          : "border-zinc-300 bg-zinc-50 text-zinc-600 hover:border-red-300 hover:text-red-600"
                      }`}
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
                      className={`max-h-40 max-w-full rounded-lg border object-contain ${
                        isDark ? "border-zinc-800 bg-zinc-950" : "border-zinc-200 bg-zinc-100"
                      }`}
                    />
                    <span className={`text-[11px] font-mono ${isDark ? "text-zinc-400" : "text-zinc-500"}`}>
                      {clip.text_content}
                    </span>
                  </div>
                ) : (
                  <pre
                    className={`mt-2 max-h-36 overflow-auto whitespace-pre-wrap break-words rounded-lg border p-2.5 text-xs ${
                      clip.clip_type === "code" ? "font-mono" : ""
                    } ${
                      isDark
                        ? "border-zinc-800/80 bg-zinc-900/70 text-zinc-200"
                        : "border-zinc-200 bg-zinc-50 text-zinc-800"
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