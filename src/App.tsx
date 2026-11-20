import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface Phase1Status {
  version: string;
  db_path: string;
  images_dir: string;
  tables: string[];
  paused: boolean;
}

export default function App() {
  const [status, setStatus] = useState<Phase1Status | null>(null);
  const [error, setError] = useState<string | null>(null);

  const loadStatus = async () => {
    try {
      const data = await invoke<Phase1Status>("get_phase1_status");
      setStatus(data);
      setError(null);
    } catch (err) {
      setError(String(err));
    }
  };

  useEffect(() => {
    loadStatus();

    const unlistenPromise = listen<boolean>("klip://paused-changed", (event) => {
      setStatus((prev) => (prev ? { ...prev, paused: event.payload } : prev));
    });

    return () => {
      unlistenPromise.then((unlisten) => unlisten());
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

  return (
    <div className="flex h-full flex-col bg-[#0e1015] text-zinc-100">
      <header className="flex items-center justify-between border-b border-zinc-800/80 px-6 py-4">
        <div className="flex items-center gap-3">
          <div className="flex h-9 w-9 items-center justify-center rounded-lg border border-zinc-700/70 bg-zinc-900">
            <svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true">
              <path
                d="M7 4v16M7 12l8-7M10.5 9.5L17 20"
                stroke="#f4f4f5"
                strokeWidth="2.2"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
          </div>
          <div>
            <h1 className="text-base font-semibold tracking-tight text-zinc-100">
              Klip
            </h1>
            <p className="text-xs text-zinc-400">
              Local-only Windows Clipboard Manager
            </p>
          </div>
        </div>

        <span className="rounded-md border border-zinc-800 bg-zinc-900 px-2.5 py-1 text-xs font-medium text-zinc-300">
          Phase 1 Ready
        </span>
      </header>

      <main className="flex-1 overflow-auto p-6">
        <div className="mx-auto max-w-2xl space-y-4">
          {error && (
            <div className="rounded-lg border border-red-900/70 bg-red-950/40 p-3 text-xs text-red-300">
              {error}
            </div>
          )}

          <section className="rounded-xl border border-zinc-800/90 bg-[#14171f] p-5">
            <div className="flex items-center justify-between">
              <div>
                <h2 className="text-sm font-semibold text-zinc-100">
                  Capture State
                </h2>
                <p className="mt-0.5 text-xs text-zinc-400">
                  Synchronized with the system tray icon and menu.
                </p>
              </div>
              <button
                type="button"
                onClick={handleTogglePause}
                className={`rounded-lg border px-3.5 py-1.5 text-xs font-medium transition-colors ${
                  status?.paused
                    ? "border-amber-700/60 bg-amber-950/40 text-amber-200 hover:bg-amber-900/50"
                    : "border-emerald-700/60 bg-emerald-950/40 text-emerald-200 hover:bg-emerald-900/50"
                }`}
              >
                {status?.paused ? "Paused â€” Resume Saving" : "Active â€” Pause Saving"}
              </button>
            </div>
          </section>

          <section className="rounded-xl border border-zinc-800/90 bg-[#14171f] p-5 space-y-3">
            <h2 className="text-sm font-semibold text-zinc-100">
              Storage &amp; SQLite Schema
            </h2>
            {status ? (
              <div className="space-y-2.5 text-xs">
                <div>
                  <span className="text-zinc-400">Version: </span>
                  <span className="font-mono text-zinc-200">v{status.version}</span>
                </div>
                <div>
                  <span className="text-zinc-400">SQLite Database: </span>
                  <span className="break-all font-mono text-zinc-200">{status.db_path}</span>
                </div>
                <div>
                  <span className="text-zinc-400">Images Directory: </span>
                  <span className="break-all font-mono text-zinc-200">{status.images_dir}</span>
                </div>
                <div>
                  <span className="text-zinc-400">Initialized Tables:</span>
                  <div className="mt-2 flex flex-wrap gap-1.5">
                    {status.tables.map((table) => (
                      <span
                        key={table}
                        className="rounded border border-zinc-700/80 bg-zinc-900 px-2 py-0.5 font-mono text-[11px] text-zinc-200"
                      >
                        {table}
                      </span>
                    ))}
                  </div>
                </div>
              </div>
            ) : (
              <p className="text-xs text-zinc-400">Initializing...</p>
            )}
          </section>

          <p className="text-xs text-zinc-500">
            Closing this window hides Klip back to the system tray. Use &ldquo;Quit Klip&rdquo; in the tray menu to exit completely.
          </p>
        </div>
      </main>
    </div>
  );
}