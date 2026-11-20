import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
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

const SYSTEM_DEFAULT_FONT = "Segoe UI, Nirmala UI, sans-serif";

export default function App() {
  const [status, setStatus] = useState<Phase1Status | null>(null);
  const [fonts, setFonts] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);

  const loadData = async () => {
    try {
      const [data, installedFonts] = await Promise.all([
        invoke<Phase1Status>("get_phase1_status"),
        invoke<string[]>("list_installed_fonts")
      ]);
      setStatus(data);
      setFonts(installedFonts);
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  };

  useEffect(() => {
    loadData();

    const unlistenPause = listen<boolean>("klip://paused-changed", (event) => {
      setStatus((prev) => (prev ? { ...prev, paused: event.payload } : prev));
    });

    const unlistenTheme = listen<"dark" | "light">("klip://theme-changed", (event) => {
      setStatus((prev) => (prev ? { ...prev, theme: event.payload } : prev));
    });

    return () => {
      unlistenPause.then((fn) => fn());
      unlistenTheme.then((fn) => fn());
    };
  }, []);

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

  const isDark = (status?.theme ?? "dark") === "dark";
  const activeFont = status?.font_family || SYSTEM_DEFAULT_FONT;

  return (
    <div
      style={{ fontFamily: activeFont }}
      className={`flex h-full flex-col ${
        isDark ? "bg-[#0e1015] text-zinc-100" : "bg-zinc-50 text-zinc-900"
      }`}
    >
      <header
        className={`flex items-center justify-between border-b px-6 py-4 ${
          isDark ? "border-zinc-800/80" : "border-zinc-200 bg-white"
        }`}
      >
        <div className="flex items-center gap-3">
          <div
            className={`flex h-9 w-9 items-center justify-center rounded-lg border ${
              isDark
                ? "border-zinc-700/70 bg-zinc-900"
                : "border-zinc-300 bg-zinc-100"
            }`}
          >
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true">
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
            <h1 className="text-base font-semibold tracking-tight">Klip</h1>
            <p className={`text-xs ${isDark ? "text-zinc-400" : "text-zinc-500"}`}>
              Local-only Windows Clipboard Manager
            </p>
          </div>
        </div>

        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={handleToggleTheme}
            className={`rounded-md border px-3 py-1.5 text-xs font-medium ${
              isDark
                ? "border-zinc-700 bg-zinc-900 text-zinc-200 hover:bg-zinc-800"
                : "border-zinc-300 bg-zinc-100 text-zinc-800 hover:bg-zinc-200"
            }`}
          >
            {isDark ? "Theme: Dark" : "Theme: Light"}
          </button>
          <span
            className={`rounded-md border px-2.5 py-1 text-xs font-medium ${
              isDark
                ? "border-zinc-800 bg-zinc-900 text-zinc-300"
                : "border-zinc-300 bg-zinc-100 text-zinc-700"
            }`}
          >
            Phase 1 Ready
          </span>
        </div>
      </header>

      <main className="flex-1 overflow-auto p-6">
        <div className="mx-auto max-w-3xl space-y-4">
          {error && (
            <div className="rounded-lg border border-red-700/60 bg-red-950/30 p-3 text-xs text-red-400">
              {error}
            </div>
          )}

          {/* Capture State */}
          <section
            className={`rounded-xl border p-5 ${
              isDark ? "border-zinc-800/90 bg-[#14171f]" : "border-zinc-200 bg-white"
            }`}
          >
            <div className="flex items-center justify-between gap-4">
              <div>
                <h2 className="text-sm font-semibold">Capture State</h2>
                <p className={`mt-0.5 text-xs ${isDark ? "text-zinc-400" : "text-zinc-500"}`}>
                  Synchronized with the theme-matched system tray icon and menu.
                </p>
              </div>
              <button
                type="button"
                onClick={handleTogglePause}
                className={`rounded-lg border px-3.5 py-1.5 text-xs font-medium ${
                  status?.paused
                    ? isDark
                      ? "border-amber-700/60 bg-amber-950/40 text-amber-200 hover:bg-amber-900/50"
                      : "border-amber-400 bg-amber-50 text-amber-800 hover:bg-amber-100"
                    : isDark
                    ? "border-emerald-700/60 bg-emerald-950/40 text-emerald-200 hover:bg-emerald-900/50"
                    : "border-emerald-400 bg-emerald-50 text-emerald-800 hover:bg-emerald-100"
                }`}
              >
                {status?.paused
                  ? "Paused \u2014 Resume Saving"
                  : "Active \u2014 Pause Saving"}
              </button>
            </div>
          </section>

          {/* Typography & PC Font Picker Preview */}
          <section
            className={`rounded-xl border p-5 space-y-3 ${
              isDark ? "border-zinc-800/90 bg-[#14171f]" : "border-zinc-200 bg-white"
            }`}
          >
            <div className="flex flex-wrap items-center justify-between gap-3">
              <div>
                <h2 className="text-sm font-semibold">System &amp; PC Font Preview</h2>
                <p className={`mt-0.5 text-xs ${isDark ? "text-zinc-400" : "text-zinc-500"}`}>
                  Zero bundled fonts. Reads fonts directly from Windows Registry ({fonts.length} detected).
                </p>
              </div>
              <select
                value={activeFont}
                onChange={(e) => handleFontChange(e.target.value)}
                className={`max-w-xs rounded-lg border px-2.5 py-1.5 text-xs ${
                  isDark
                    ? "border-zinc-700 bg-zinc-900 text-zinc-100"
                    : "border-zinc-300 bg-zinc-50 text-zinc-900"
                }`}
              >
                <option value={SYSTEM_DEFAULT_FONT}>
                  Windows Default (Segoe UI + Nirmala UI)
                </option>
                {fonts.map((fontName) => (
                  <option key={fontName} value={`"${fontName}", "Nirmala UI", sans-serif`}>
                    {fontName}
                  </option>
                ))}
              </select>
            </div>

            <div
              className={`rounded-lg border p-3 text-xs space-y-1 ${
                isDark
                  ? "border-zinc-800 bg-zinc-900/70 text-zinc-200"
                  : "border-zinc-200 bg-zinc-50 text-zinc-800"
              }`}
            >
              <p>
                English UTF-8 Check: Quick brown fox {"\u2014"} Clipboard ready &ldquo;Klip&rdquo;.
              </p>
              <p>
                Bangla UTF-8 Check: {"\u0995\u09AA\u09BF \u0993 \u09AA\u09C7\u09B8\u09CD\u099F \u09AA\u09B0\u09C0\u0995\u09CD\u09B7\u09BE \u2014 \u09AC\u09BE\u0982\u09B2\u09BE \u09B8\u09A0\u09BF\u0995\u09AD\u09BE\u09AC\u09C7 \u09A6\u09C7\u0996\u09BE \u09AF\u09BE\u099A\u09CD\u099B\u09C7\u0964"}
              </p>
            </div>
          </section>

          {/* Storage & SQLite Schema */}
          <section
            className={`rounded-xl border p-5 space-y-3 ${
              isDark ? "border-zinc-800/90 bg-[#14171f]" : "border-zinc-200 bg-white"
            }`}
          >
            <h2 className="text-sm font-semibold">Storage &amp; SQLite Schema</h2>
            {status ? (
              <div className="space-y-2.5 text-xs">
                <div>
                  <span className={isDark ? "text-zinc-400" : "text-zinc-500"}>Version: </span>
                  <span className="font-mono">v{status.version}</span>
                </div>
                <div>
                  <span className={isDark ? "text-zinc-400" : "text-zinc-500"}>SQLite Database: </span>
                  <span className="break-all font-mono">{status.db_path}</span>
                </div>
                <div>
                  <span className={isDark ? "text-zinc-400" : "text-zinc-500"}>Images Directory: </span>
                  <span className="break-all font-mono">{status.images_dir}</span>
                </div>
                <div>
                  <span className={isDark ? "text-zinc-400" : "text-zinc-500"}>Tables:</span>
                  <div className="mt-1.5 flex flex-wrap gap-1.5">
                    {status.tables.map((table) => (
                      <span
                        key={table}
                        className={`rounded border px-2 py-0.5 font-mono text-[11px] ${
                          isDark
                            ? "border-zinc-700/80 bg-zinc-900 text-zinc-200"
                            : "border-zinc-300 bg-zinc-100 text-zinc-800"
                        }`}
                      >
                        {table}
                      </span>
                    ))}
                  </div>
                </div>
                <div>
                  <span className={isDark ? "text-zinc-400" : "text-zinc-500"}>
                    Seeded Settings ({status.settings.length}):
                  </span>
                  <div
                    className={`mt-1.5 max-h-40 overflow-auto rounded-lg border p-2 font-mono text-[11px] ${
                      isDark
                        ? "border-zinc-800 bg-zinc-900/70 text-zinc-300"
                        : "border-zinc-200 bg-zinc-50 text-zinc-700"
                    }`}
                  >
                    {status.settings.map((s) => (
                      <div key={s.key} className="flex gap-2 py-0.5">
                        <span className={isDark ? "text-zinc-400" : "text-zinc-500"}>{s.key}:</span>
                        <span className="break-all">{s.value || '""'}</span>
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            ) : (
              <p className={`text-xs ${isDark ? "text-zinc-400" : "text-zinc-500"}`}>
                Initializing...
              </p>
            )}
          </section>

          <p className={`text-xs ${isDark ? "text-zinc-500" : "text-zinc-500"}`}>
            Closing this window destroys the WebView2 instance so idle tray RAM stays minimal (~5-10 MB). Use &ldquo;Quit Klip&rdquo; in the tray menu to exit completely.
          </p>
        </div>
      </main>
    </div>
  );
}