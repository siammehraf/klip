import { useEffect, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ClipItem,
  ClipSourceBadge,
  formatBytes,
  formatRelative,
  typeLabel
} from "./components/ClipBadge";

interface PopupInitData {
  theme: "dark" | "light";
  font_family: string;
  paste_mode: "auto_paste" | "copy_only";
  paused: boolean;
  clips: ClipItem[];
}

export default function PopupApp() {
  const [theme, setTheme] = useState<"dark" | "light">("dark");
  const [fontFamily, setFontFamily] = useState("Segoe UI, Nirmala UI, sans-serif");
  const [pasteMode, setPasteMode] = useState<"auto_paste" | "copy_only">("auto_paste");
  const [tab, setTab] = useState<"history" | "pinned">("history");
  const [search, setSearch] = useState("");
  const [clips, setClips] = useState<ClipItem[]>([]);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [keepOpen, setKeepOpen] = useState(false);

  const searchRef = useRef<HTMLInputElement>(null);
  const rowRefs = useRef<(HTMLButtonElement | null)[]>([]);

  useEffect(() => {
    invoke<PopupInitData>("get_popup_init_data")
      .then((data) => {
        setTheme(data.theme === "light" ? "light" : "dark");
        setFontFamily(data.font_family || "Segoe UI, Nirmala UI, sans-serif");
        setPasteMode(data.paste_mode === "copy_only" ? "copy_only" : "auto_paste");
        setClips(data.clips);
        setSelectedIndex(0);
      })
      .catch(() => {});

    const unlistenAdded = listen<ClipItem>("klip://clip-added", (event) => {
      if (tab === "history") {
        setClips((prev) => {
          if (prev.some((c) => c.id === event.payload.id)) return prev;
          return [event.payload, ...prev].slice(0, 150);
        });
      }
    });

    const unlistenDeleted = listen<number>("klip://clip-deleted", (event) => {
      setClips((prev) => prev.filter((c) => c.id !== event.payload));
    });

    const unlistenPinned = listen<[number, boolean]>("klip://clip-pinned", (event) => {
      const [id, isPinned] = event.payload;
      setClips((prev) =>
        prev
          .map((c) => (c.id === id ? { ...c, is_pinned: isPinned } : c))
          .filter((c) => (tab === "pinned" ? c.is_pinned : true))
      );
    });

    const timer = setTimeout(() => {
      searchRef.current?.focus();
    }, 20);

    return () => {
      clearTimeout(timer);
      unlistenAdded.then((fn) => fn());
      unlistenDeleted.then((fn) => fn());
      unlistenPinned.then((fn) => fn());
    };
  }, [tab]);

  useEffect(() => {
    invoke<ClipItem[]>("list_clips", {
      search,
      pinnedOnly: tab === "pinned",
      limit: 150
    })
      .then((list) => {
        setClips(list);
        setSelectedIndex(0);
      })
      .catch(() => {});
  }, [search, tab]);

  useEffect(() => {
    if (selectedIndex >= clips.length && clips.length > 0) {
      setSelectedIndex(clips.length - 1);
    }
  }, [clips.length, selectedIndex]);

  useEffect(() => {
    rowRefs.current[selectedIndex]?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);

  const activeClip = clips[selectedIndex] ?? null;

  const handlePaste = async (clip: ClipItem, plainOnly = false) => {
    await invoke("paste_clip", { clipId: clip.id, plainOnly });
  };

  const handlePin = async (clip: ClipItem) => {
    const nextPinned = await invoke<boolean>("toggle_pin_clip", { clipId: clip.id });
    setClips((prev) =>
      prev
        .map((c) => (c.id === clip.id ? { ...c, is_pinned: nextPinned } : c))
        .filter((c) => (tab === "pinned" ? c.is_pinned : true))
    );
  };

  const handleDelete = async (clip: ClipItem) => {
    await invoke("delete_clip", { clipId: clip.id });
    setClips((prev) => prev.filter((c) => c.id !== clip.id));
  };

  const handleToggleKeepOpen = async () => {
    const next = !keepOpen;
    setKeepOpen(next);
    await invoke("set_popup_keep_open", { keepOpen: next });
  };

  const handleClose = () => {
    invoke("close_popup");
  };

  const handleStartDrag = (e: React.MouseEvent) => {
    if ((e.target as HTMLElement).closest("button")) return;
    getCurrentWindow().startDragging().catch(() => {});
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      handleClose();
      return;
    }
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (clips.length > 0) {
        setSelectedIndex((idx) => (idx + 1) % clips.length);
      }
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      if (clips.length > 0) {
        setSelectedIndex((idx) => (idx - 1 + clips.length) % clips.length);
      }
      return;
    }
    if (e.key === "Enter" && activeClip) {
      e.preventDefault();
      handlePaste(activeClip, e.shiftKey);
      return;
    }
    if (e.ctrlKey && e.key.toLowerCase() === "f") {
      e.preventDefault();
      searchRef.current?.focus();
      return;
    }
    if (e.ctrlKey && e.key.toLowerCase() === "p" && activeClip) {
      e.preventDefault();
      handlePin(activeClip);
      return;
    }
    if (e.ctrlKey && e.key === "Tab") {
      e.preventDefault();
      setTab((prev) => (prev === "history" ? "pinned" : "history"));
      return;
    }

    const searchFocused = document.activeElement === searchRef.current;

    if (e.key === "Delete" && !searchFocused && activeClip) {
      e.preventDefault();
      handleDelete(activeClip);
      return;
    }

    if (/^[1-9]$/.test(e.key) && (e.ctrlKey || e.altKey || !searchFocused)) {
      const idx = Number(e.key) - 1;
      if (clips[idx]) {
        e.preventDefault();
        handlePaste(clips[idx], e.shiftKey);
      }
    }
  };

  const isDark = theme === "dark";

  return (
    <div
      tabIndex={-1}
      onKeyDown={handleKeyDown}
      style={{ fontFamily }}
      className={`flex h-full flex-col select-none border ${
        isDark
          ? "border-zinc-800 bg-[#0e1015] text-zinc-100"
          : "border-zinc-300 bg-zinc-50 text-zinc-900"
      }`}
    >
      <header
        onMouseDown={handleStartDrag}
        className={`flex items-center justify-between border-b px-3 py-2 cursor-move ${
          isDark ? "border-zinc-800 bg-[#12151d]" : "border-zinc-200 bg-white"
        }`}
      >
        <div className="flex items-center gap-1.5">
          <div
            className={`mr-1 flex h-6 w-6 items-center justify-center rounded border ${
              isDark ? "border-zinc-700 bg-zinc-900" : "border-zinc-300 bg-zinc-100"
            }`}
          >
            <svg width="13" height="13" viewBox="0 0 24 24" fill="none" aria-hidden="true">
              <path
                d="M7 4v16M7 12l8-7M10.5 9.5L17 20"
                stroke={isDark ? "#f4f4f5" : "#18181b"}
                strokeWidth="2.4"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
          </div>

          <button
            type="button"
            onClick={() => setTab("history")}
            className={`rounded px-2.5 py-1 text-xs font-medium ${
              tab === "history"
                ? isDark
                  ? "bg-zinc-800 text-white"
                  : "bg-zinc-900 text-white"
                : isDark
                ? "text-zinc-400 hover:text-zinc-200"
                : "text-zinc-600 hover:text-zinc-900"
            }`}
          >
            History
          </button>
          <button
            type="button"
            onClick={() => setTab("pinned")}
            className={`rounded px-2.5 py-1 text-xs font-medium ${
              tab === "pinned"
                ? isDark
                  ? "bg-zinc-800 text-white"
                  : "bg-zinc-900 text-white"
                : isDark
                ? "text-zinc-400 hover:text-zinc-200"
                : "text-zinc-600 hover:text-zinc-900"
            }`}
          >
            Pinned
          </button>
        </div>

        <div className="flex items-center gap-1.5">
          <button
            type="button"
            onClick={handleToggleKeepOpen}
            title="Keep popup open when clicking outside"
            className={`rounded border px-2 py-0.5 text-[11px] font-medium ${
              keepOpen
                ? isDark
                  ? "border-sky-600 bg-sky-950/60 text-sky-200"
                  : "border-sky-500 bg-sky-50 text-sky-700"
                : isDark
                ? "border-zinc-700 bg-zinc-900 text-zinc-400 hover:text-zinc-200"
                : "border-zinc-300 bg-zinc-100 text-zinc-600 hover:text-zinc-900"
            }`}
          >
            {keepOpen ? "Pinned Open" : "Keep Open"}
          </button>
          <button
            type="button"
            onClick={handleClose}
            title="Close (Esc)"
            className={`rounded border px-2 py-0.5 text-[11px] ${
              isDark
                ? "border-zinc-700 bg-zinc-900 text-zinc-400 hover:text-zinc-100"
                : "border-zinc-300 bg-zinc-100 text-zinc-600 hover:text-zinc-900"
            }`}
          >
            Esc
          </button>
        </div>
      </header>

      <div
        className={`flex items-center gap-2 border-b px-3 py-2 ${
          isDark ? "border-zinc-800/80 bg-[#0e1015]" : "border-zinc-200 bg-zinc-50"
        }`}
      >
        <input
          ref={searchRef}
          type="text"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Search clips (English, Bangla, app, or site)..."
          className={`flex-1 rounded border px-2.5 py-1 text-xs outline-none ${
            isDark
              ? "border-zinc-700 bg-zinc-900 text-zinc-100 placeholder:text-zinc-500 focus:border-zinc-500"
              : "border-zinc-300 bg-white text-zinc-900 placeholder:text-zinc-400 focus:border-zinc-500"
          }`}
        />
        <span className="text-[10px] text-zinc-500">
          {"\u2191\u2193"} Move {"\u2022"} Enter Paste {"\u2022"} Ctrl+1-9
        </span>
      </div>

      <div className="flex min-h-0 flex-1">
        <div
          className={`w-[46%] overflow-y-auto border-r p-1.5 space-y-1 ${
            isDark ? "border-zinc-800/90 bg-[#0e1015]" : "border-zinc-200 bg-zinc-50"
          }`}
        >
          {clips.length === 0 ? (
            <div className="p-6 text-center text-xs text-zinc-500">
              {tab === "pinned" ? "No pinned clips" : "No matching clips"}
            </div>
          ) : (
            clips.map((clip, idx) => {
              const selected = idx === selectedIndex;
              const quickNum = idx < 9 ? idx + 1 : null;
              const snippet =
                clip.clip_type === "image"
                  ? `Image (${clip.text_content ?? "PNG"})`
                  : (clip.text_content ?? "").replace(/\s+/g, " ").trim() || "(Rich HTML)";

              return (
                <button
                  key={clip.id}
                  ref={(el) => {
                    rowRefs.current[idx] = el;
                  }}
                  type="button"
                  onClick={() => setSelectedIndex(idx)}
                  onDoubleClick={() => handlePaste(clip, false)}
                  className={`w-full rounded-lg border px-2.5 py-2 text-left ${
                    selected
                      ? isDark
                        ? "border-sky-500/70 bg-[#182030]"
                        : "border-sky-500 bg-sky-50"
                      : isDark
                      ? "border-zinc-800/70 bg-[#14171f] hover:border-zinc-700"
                      : "border-zinc-200 bg-white hover:border-zinc-300"
                  }`}
                >
                  <div className="flex items-center justify-between gap-1.5 text-[10px]">
                    <div className="flex min-w-0 items-center gap-1.5">
                      {quickNum !== null && (
                        <span
                          className={`inline-flex h-4 w-4 shrink-0 items-center justify-center rounded font-mono text-[10px] font-semibold ${
                            selected
                              ? "bg-sky-500 text-white"
                              : isDark
                              ? "bg-zinc-800 text-zinc-400"
                              : "bg-zinc-200 text-zinc-700"
                          }`}
                        >
                          {quickNum}
                        </span>
                      )}
                      <ClipSourceBadge clip={clip} isDark={isDark} compact={true} />
                    </div>
                    <div className="flex shrink-0 items-center gap-1">
                      {clip.is_pinned && (
                        <span className="text-[10px] font-semibold text-amber-400">PIN</span>
                      )}
                      <span className={isDark ? "text-zinc-500" : "text-zinc-400"}>
                        {formatRelative(clip.created_at)}
                      </span>
                    </div>
                  </div>

                  <div className="mt-1 flex items-center gap-1.5">
                    {clip.clip_type === "color" && clip.text_content && (
                      <span
                        className="inline-block h-3 w-3 shrink-0 rounded-sm border border-zinc-500/60"
                        style={{ backgroundColor: clip.text_content.trim() }}
                      />
                    )}
                    <p
                      className={`truncate text-xs ${
                        isDark ? "text-zinc-200" : "text-zinc-800"
                      }`}
                    >
                      {snippet}
                    </p>
                  </div>
                </button>
              );
            })
          )}
        </div>

        <div className={`flex w-[54%] flex-col ${isDark ? "bg-[#12151d]" : "bg-white"}`}>
          {activeClip ? (
            <>
              <div
                className={`flex items-center justify-between gap-2 border-b px-3 py-2 text-[11px] ${
                  isDark ? "border-zinc-800/80" : "border-zinc-200"
                }`}
              >
                <div className="flex min-w-0 items-center gap-2">
                  <ClipSourceBadge clip={activeClip} isDark={isDark} compact={true} />
                  <span
                    className={`rounded border px-1.5 py-0.5 text-[10px] ${
                      isDark
                        ? "border-zinc-700 bg-zinc-900 text-zinc-300"
                        : "border-zinc-300 bg-zinc-100 text-zinc-700"
                    }`}
                  >
                    {typeLabel(activeClip.clip_type)}
                  </span>
                </div>

                <div className="flex shrink-0 items-center gap-1">
                  <button
                    type="button"
                    onClick={() => handlePin(activeClip)}
                    className={`rounded border px-2 py-0.5 text-[11px] ${
                      activeClip.is_pinned
                        ? isDark
                          ? "border-amber-600/70 bg-amber-950/50 text-amber-200"
                          : "border-amber-400 bg-amber-50 text-amber-800"
                        : isDark
                        ? "border-zinc-700 bg-zinc-900 text-zinc-300 hover:bg-zinc-800"
                        : "border-zinc-300 bg-zinc-100 text-zinc-700 hover:bg-zinc-200"
                    }`}
                  >
                    {activeClip.is_pinned ? "Unpin" : "Pin"}
                  </button>
                  <button
                    type="button"
                    onClick={() => handleDelete(activeClip)}
                    className={`rounded border px-2 py-0.5 text-[11px] ${
                      isDark
                        ? "border-zinc-700 bg-zinc-900 text-zinc-400 hover:border-red-800 hover:text-red-300"
                        : "border-zinc-300 bg-zinc-100 text-zinc-600 hover:border-red-300 hover:text-red-600"
                    }`}
                  >
                    Delete
                  </button>
                </div>
              </div>

              <div className="min-h-0 flex-1 overflow-auto p-3 select-text">
                {activeClip.clip_type === "image" && activeClip.image_path ? (
                  <div className="flex h-full flex-col items-center justify-center gap-2">
                    <img
                      src={convertFileSrc(activeClip.image_path)}
                      alt="Clip preview"
                      className={`max-h-[230px] max-w-full rounded border object-contain ${
                        isDark ? "border-zinc-800 bg-zinc-950" : "border-zinc-200 bg-zinc-100"
                      }`}
                    />
                    <span className="font-mono text-[11px] text-zinc-500">
                      {activeClip.text_content} {"\u2022"} {formatBytes(activeClip.content_size)}
                    </span>
                  </div>
                ) : activeClip.clip_type === "color" && activeClip.text_content ? (
                  <div className="space-y-3">
                    <div
                      className="h-24 w-full rounded-lg border border-zinc-600/40"
                      style={{ backgroundColor: activeClip.text_content.trim() }}
                    />
                    <pre
                      className={`rounded border p-2.5 font-mono text-xs ${
                        isDark
                          ? "border-zinc-800 bg-zinc-900 text-zinc-100"
                          : "border-zinc-200 bg-zinc-50 text-zinc-900"
                      }`}
                    >
                      {activeClip.text_content}
                    </pre>
                  </div>
                ) : (
                  <pre
                    className={`whitespace-pre-wrap break-words text-xs leading-relaxed ${
                      activeClip.clip_type === "code" ? "font-mono" : ""
                    } ${isDark ? "text-zinc-200" : "text-zinc-800"}`}
                  >
                    {activeClip.text_content ?? "(Rich HTML content)"}
                  </pre>
                )}
              </div>

              <div
                className={`flex items-center justify-between border-t px-3 py-2 ${
                  isDark ? "border-zinc-800/80" : "border-zinc-200"
                }`}
              >
                <span className="text-[10px] text-zinc-500">
                  {formatBytes(activeClip.content_size)}
                  {activeClip.source_site ? ` \u2022 ${activeClip.source_site}` : ""}
                </span>
                <div className="flex items-center gap-1.5">
                  {activeClip.html_content && (
                    <button
                      type="button"
                      onClick={() => handlePaste(activeClip, true)}
                      title="Paste without HTML formatting (Shift+Enter)"
                      className={`rounded border px-2.5 py-1 text-[11px] ${
                        isDark
                          ? "border-zinc-700 bg-zinc-900 text-zinc-300 hover:bg-zinc-800"
                          : "border-zinc-300 bg-zinc-100 text-zinc-700 hover:bg-zinc-200"
                      }`}
                    >
                      Plain
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={() => handlePaste(activeClip, false)}
                    className="rounded bg-sky-600 px-3 py-1 text-xs font-medium text-white hover:bg-sky-500"
                  >
                    {pasteMode === "copy_only" ? "Copy" : "Paste"}
                  </button>
                </div>
              </div>
            </>
          ) : (
            <div className="flex flex-1 items-center justify-center text-xs text-zinc-500">
              Select a clip to preview
            </div>
          )}
        </div>
      </div>
    </div>
  );
}