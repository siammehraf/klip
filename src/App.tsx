import { useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface Phase1Status {
  version: string;
  paused: boolean;
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
  source_site: string | null;
  is_pinned: boolean;
  created_at: number;
}

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

function cleanDomain(sourceSite: string): string {
  return sourceSite.replace(/^site:/i, "").toLowerCase();
}

function siteDisplayName(domain: string): string {
  if (domain === "facebook.com" || domain.endsWith(".facebook.com")) return "Facebook";
  if (domain === "github.com" || domain.endsWith(".github.com")) return "GitHub";
  if (domain === "youtube.com" || domain === "youtu.be" || domain.endsWith(".youtube.com")) return "YouTube";
  if (domain === "mail.google.com") return "Gmail";
  if (domain === "google.com" || domain.endsWith(".google.com")) return "Google";
  if (domain === "x.com" || domain === "twitter.com") return "X";
  if (domain === "reddit.com" || domain.endsWith(".reddit.com")) return "Reddit";
  if (domain === "linkedin.com" || domain.endsWith(".linkedin.com")) return "LinkedIn";
  if (domain === "chatgpt.com" || domain === "chat.openai.com") return "ChatGPT";
  return domain;
}

function SiteBadgeIcon({ domain }: { domain: string }) {
  if (domain === "facebook.com" || domain.endsWith(".facebook.com")) {
    return (
      <span className="inline-flex h-4 w-4 items-center justify-center rounded-sm bg-[#1877F2] text-white">
        <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path d="M14 13.5h2.5l1-4H14v-2c0-1.03 0-2 2-2h1.5V2.14c-.326-.043-1.557-.14-2.857-.14C11.928 2 10 3.657 10 6.7v2.8H7v4h3V22h4v-8.5z" />
        </svg>
      </span>
    );
  }
  if (domain === "github.com" || domain.endsWith(".github.com")) {
    return (
      <span className="inline-flex h-4 w-4 items-center justify-center rounded-sm bg-zinc-200 text-zinc-900">
        <svg width="11" height="11" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path d="M12 2C6.477 2 2 6.484 2 12.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.53 1.032 1.53 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0112 6.844c.85.004 1.705.115 2.504.337 1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.019 10.019 0 0022 12.017C22 6.484 17.522 2 12 2z" />
        </svg>
      </span>
    );
  }
  if (domain === "youtube.com" || domain === "youtu.be" || domain.endsWith(".youtube.com")) {
    return (
      <span className="inline-flex h-4 w-4 items-center justify-center rounded-sm bg-[#FF0000] text-white">
        <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path d="M8 5v14l11-7z" />
        </svg>
      </span>
    );
  }
  if (domain === "mail.google.com") {
    return (
      <span className="inline-flex h-4 w-4 items-center justify-center rounded-sm bg-[#EA4335] text-white">
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" aria-hidden="true">
          <rect x="3" y="5" width="18" height="14" rx="2" />
          <path d="M3 7l9 6 9-6" />
        </svg>
      </span>
    );
  }
  if (domain === "google.com" || domain.endsWith(".google.com")) {
    return (
      <span className="inline-flex h-4 w-4 items-center justify-center rounded-sm bg-[#4285F4] text-[10px] font-bold text-white">
        G
      </span>
    );
  }
  if (domain === "x.com" || domain === "twitter.com") {
    return (
      <span className="inline-flex h-4 w-4 items-center justify-center rounded-sm bg-zinc-950 border border-zinc-700 text-[10px] font-bold text-white">
        X
      </span>
    );
  }
  if (domain === "reddit.com" || domain.endsWith(".reddit.com")) {
    return (
      <span className="inline-flex h-4 w-4 items-center justify-center rounded-sm bg-[#FF4500] text-[10px] font-bold text-white">
        r/
      </span>
    );
  }
  if (domain === "linkedin.com" || domain.endsWith(".linkedin.com")) {
    return (
      <span className="inline-flex h-4 w-4 items-center justify-center rounded-sm bg-[#0A66C2] text-[9px] font-bold text-white">
        in
      </span>
    );
  }
  if (domain === "chatgpt.com" || domain === "chat.openai.com") {
    return (
      <span className="inline-flex h-4 w-4 items-center justify-center rounded-sm bg-[#10A37F] text-white">
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" aria-hidden="true">
          <circle cx="12" cy="12" r="8" />
          <path d="M12 4v16M4 12h16" />
        </svg>
      </span>
    );
  }

  const initial = (domain[0] ?? "W").toUpperCase();
  return (
    <span className="inline-flex h-4 w-4 items-center justify-center rounded-sm border border-zinc-700 bg-zinc-800 text-[9px] font-semibold text-zinc-200">
      {initial}
    </span>
  );
}

export default function App() {
  const [paused, setPaused] = useState(false);
  const [version, setVersion] = useState("0.1.0");
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
        setExcludedCount(excluded.length);
        setClips(initialClips);
      })
      .catch((err) => setError(String(err)));

    const unlistenPause = listen<boolean>("klip://paused-changed", (event) => {
      setPaused(event.payload);
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
      unlistenAdded.then((fn) => fn());
      unlistenPruned.then((fn) => fn());
    };
  }, []);

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

  return (
    <div className="flex h-full flex-col bg-[#0e1015] text-zinc-100">
      {/* Compact Header */}
      <header className="flex items-center justify-between gap-3 border-b border-zinc-800/80 px-5 py-3">
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
            Klip <span className="font-normal text-zinc-500">v{version} {"\u2014"} Phase 2B Capture Feed</span>
          </h1>
        </div>

        <div className="flex items-center gap-2">
          <span className="rounded-md border border-zinc-800 bg-zinc-900/80 px-2.5 py-1 text-[11px] text-zinc-400">
            {excludedCount} excluded {excludedCount === 1 ? "app" : "apps"}
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

      {/* Search & Filter Bar */}
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

      {/* Live Feed */}
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
              Copy text, Bangla Unicode, website content from Brave/Chrome/Edge, colors, code, or screenshots (Win+Shift+S).
            </p>
          </div>
        ) : (
          <div className="mx-auto max-w-4xl space-y-2.5">
            {clips.map((clip) => {
              const domain = clip.source_site ? cleanDomain(clip.source_site) : null;
              return (
                <article
                  key={clip.id}
                  className="rounded-xl border border-zinc-800/90 bg-[#14171f] p-3.5"
                >
                  <div className="flex items-center justify-between gap-3">
                    <div className="flex flex-wrap items-center gap-2 text-xs">
                      {domain ? (
                        <>
                          <SiteBadgeIcon domain={domain} />
                          <span className="font-medium text-zinc-100">
                            {siteDisplayName(domain)}
                          </span>
                          <span className="rounded border border-zinc-800 bg-zinc-900/90 px-1.5 py-0.5 font-mono text-[10px] text-zinc-400">
                            {clip.source_site}
                          </span>
                          {clip.source_app && (
                            <span className="text-[11px] text-zinc-500">
                              via {clip.source_app}
                            </span>
                          )}
                        </>
                      ) : (
                        <>
                          {clip.source_icon_path ? (
                            <img
                              src={convertFileSrc(clip.source_icon_path)}
                              alt=""
                              className="h-4 w-4 rounded-sm object-contain"
                            />
                          ) : (
                            <svg
                              width="14"
                              height="14"
                              viewBox="0 0 24 24"
                              fill="none"
                              className="text-zinc-500"
                              aria-hidden="true"
                            >
                              <rect
                                x="3"
                                y="4"
                                width="18"
                                height="16"
                                rx="2"
                                stroke="currentColor"
                                strokeWidth="1.8"
                              />
                              <path d="M3 9h18" stroke="currentColor" strokeWidth="1.8" />
                            </svg>
                          )}
                          <span className="font-medium text-zinc-200">
                            {clip.source_app ?? "Unknown app"}
                          </span>
                        </>
                      )}

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
              );
            })}
          </div>
        )}
      </main>
    </div>
  );
}