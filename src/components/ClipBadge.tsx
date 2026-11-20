import { convertFileSrc } from "@tauri-apps/api/core";

export interface ClipItem {
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

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

export function formatRelative(ts: number): string {
  const diffSec = Math.max(0, Math.floor((Date.now() - ts) / 1000));
  if (diffSec < 60) return `${diffSec}s`;
  const diffMin = Math.floor(diffSec / 60);
  if (diffMin < 60) return `${diffMin}m`;
  const diffHr = Math.floor(diffMin / 60);
  if (diffHr < 24) return `${diffHr}h`;
  return `${Math.floor(diffHr / 24)}d`;
}

export function typeLabel(clipType: string): string {
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

export function cleanDomain(sourceSite: string): string {
  return sourceSite.replace(/^site:/i, "").toLowerCase();
}

function matchSite(domain: string, root: string): boolean {
  return domain === root || domain.endsWith(`.${root}`);
}

export function siteDisplayName(domain: string): string {
  if (matchSite(domain, "facebook.com")) return "Facebook";
  if (matchSite(domain, "github.com")) return "GitHub";
  if (matchSite(domain, "youtube.com") || domain === "youtu.be") return "YouTube";
  if (domain === "mail.google.com") return "Gmail";
  if (matchSite(domain, "google.com")) return "Google";
  if (matchSite(domain, "x.com") || matchSite(domain, "twitter.com")) return "X";
  if (matchSite(domain, "reddit.com")) return "Reddit";
  if (matchSite(domain, "linkedin.com")) return "LinkedIn";
  if (matchSite(domain, "chatgpt.com") || domain === "chat.openai.com") return "ChatGPT";
  if (matchSite(domain, "claude.ai")) return "Claude";
  if (matchSite(domain, "todoist.com")) return "Todoist";
  if (matchSite(domain, "discord.com")) return "Discord";
  if (matchSite(domain, "notion.so")) return "Notion";
  if (matchSite(domain, "stackoverflow.com")) return "Stack Overflow";
  if (matchSite(domain, "twitch.tv")) return "Twitch";
  if (matchSite(domain, "whatsapp.com")) return "WhatsApp";
  if (matchSite(domain, "telegram.org") || domain === "t.me") return "Telegram";
  if (matchSite(domain, "instagram.com")) return "Instagram";
  if (matchSite(domain, "netflix.com")) return "Netflix";
  if (matchSite(domain, "wikipedia.org")) return "Wikipedia";
  if (matchSite(domain, "figma.com")) return "Figma";
  if (matchSite(domain, "vercel.com")) return "Vercel";
  return domain;
}

function KnownSiteIcon({ domain }: { domain: string }) {
  if (matchSite(domain, "facebook.com")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#1877F2] text-white">
        <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path d="M14 13.5h2.5l1-4H14v-2c0-1.03 0-2 2-2h1.5V2.14c-.326-.043-1.557-.14-2.857-.14C11.928 2 10 3.657 10 6.7v2.8H7v4h3V22h4v-8.5z" />
        </svg>
      </span>
    );
  }
  if (matchSite(domain, "github.com")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-zinc-200 text-zinc-900">
        <svg width="11" height="11" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path d="M12 2C6.477 2 2 6.484 2 12.017c0 4.425 2.865 8.18 6.839 9.504.5.092.682-.217.682-.483 0-.237-.008-.868-.013-1.703-2.782.605-3.369-1.343-3.369-1.343-.454-1.158-1.11-1.466-1.11-1.466-.908-.62.069-.608.069-.608 1.003.07 1.53 1.032 1.53 1.032.892 1.53 2.341 1.088 2.91.832.092-.647.35-1.088.636-1.338-2.22-.253-4.555-1.113-4.555-4.951 0-1.093.39-1.988 1.029-2.688-.103-.253-.446-1.272.098-2.65 0 0 .84-.27 2.75 1.026A9.564 9.564 0 0112 6.844c.85.004 1.705.115 2.504.337 1.909-1.296 2.747-1.027 2.747-1.027.546 1.379.202 2.398.1 2.651.64.7 1.028 1.595 1.028 2.688 0 3.848-2.339 4.695-4.566 4.943.359.309.678.92.678 1.855 0 1.338-.012 2.419-.012 2.747 0 .268.18.58.688.482A10.019 10.019 0 0022 12.017C22 6.484 17.522 2 12 2z" />
        </svg>
      </span>
    );
  }
  if (matchSite(domain, "youtube.com") || domain === "youtu.be") {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#FF0000] text-white">
        <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path d="M8 5v14l11-7z" />
        </svg>
      </span>
    );
  }
  if (domain === "mail.google.com") {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#EA4335] text-white">
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" aria-hidden="true">
          <rect x="3" y="5" width="18" height="14" rx="2" />
          <path d="M3 7l9 6 9-6" />
        </svg>
      </span>
    );
  }
  if (matchSite(domain, "google.com")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#4285F4] text-[10px] font-bold text-white">
        G
      </span>
    );
  }
  if (matchSite(domain, "x.com") || matchSite(domain, "twitter.com")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm border border-zinc-700 bg-zinc-950 text-[10px] font-bold text-white">
        X
      </span>
    );
  }
  if (matchSite(domain, "reddit.com")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#FF4500] text-[9px] font-bold text-white">
        r/
      </span>
    );
  }
  if (matchSite(domain, "linkedin.com")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#0A66C2] text-[9px] font-bold text-white">
        in
      </span>
    );
  }
  if (matchSite(domain, "chatgpt.com") || domain === "chat.openai.com") {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#10A37F] text-white">
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" aria-hidden="true">
          <circle cx="12" cy="12" r="8" />
          <path d="M12 4v16M4 12h16" />
        </svg>
      </span>
    );
  }
  if (matchSite(domain, "claude.ai")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#D97757] text-[9px] font-bold text-white">
        C
      </span>
    );
  }
  if (matchSite(domain, "todoist.com")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#E44332] text-white">
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" aria-hidden="true">
          <path d="M5 12l4 4L19 6" />
        </svg>
      </span>
    );
  }
  if (matchSite(domain, "discord.com")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#5865F2] text-[9px] font-bold text-white">
        D
      </span>
    );
  }
  if (matchSite(domain, "notion.so")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm border border-zinc-400 bg-white text-[9px] font-bold text-zinc-900">
        N
      </span>
    );
  }
  if (matchSite(domain, "stackoverflow.com")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#F48024] text-[9px] font-bold text-white">
        SO
      </span>
    );
  }
  if (matchSite(domain, "twitch.tv")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#9146FF] text-[9px] font-bold text-white">
        Tw
      </span>
    );
  }
  if (matchSite(domain, "whatsapp.com")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#25D366] text-[9px] font-bold text-white">
        W
      </span>
    );
  }
  if (matchSite(domain, "telegram.org") || domain === "t.me") {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#26A5E4] text-[9px] font-bold text-white">
        Tg
      </span>
    );
  }
  if (matchSite(domain, "instagram.com")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#E4405F] text-[9px] font-bold text-white">
        Ig
      </span>
    );
  }
  if (matchSite(domain, "netflix.com")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#E50914] text-[10px] font-bold text-white">
        N
      </span>
    );
  }
  if (matchSite(domain, "wikipedia.org")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-zinc-200 text-[9px] font-serif font-bold text-zinc-900">
        W
      </span>
    );
  }
  if (matchSite(domain, "figma.com")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-[#A259FF] text-[9px] font-bold text-white">
        F
      </span>
    );
  }
  if (matchSite(domain, "vercel.com")) {
    return (
      <span className="inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-sm bg-black border border-zinc-700 text-white">
        <svg width="9" height="9" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path d="M12 4l9 16H3z" />
        </svg>
      </span>
    );
  }
  return null;
}

export function ClipSourceBadge({
  clip,
  isDark,
  compact = false
}: {
  clip: ClipItem;
  isDark: boolean;
  compact?: boolean;
}) {
  if (clip.source_site) {
    const domain = cleanDomain(clip.source_site);
    const known = KnownSiteIcon({ domain });

    return (
      <span className="flex min-w-0 items-center gap-1.5">
        {known ? (
          known
        ) : clip.source_icon_path ? (
          <img
            src={convertFileSrc(clip.source_icon_path)}
            alt=""
            className="h-4 w-4 shrink-0 rounded-sm object-contain"
          />
        ) : (
          <svg
            width="14"
            height="14"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.8"
            className="shrink-0 text-zinc-400"
            aria-hidden="true"
          >
            <circle cx="12" cy="12" r="9" />
            <path d="M3 12h18M12 3a14 14 0 010 18M12 3a14 14 0 000 18" />
          </svg>
        )}
        <span className="truncate font-medium">{siteDisplayName(domain)}</span>
        {!compact && (
          <>
            <span
              className={`rounded border px-1.5 py-0.5 font-mono text-[10px] ${
                isDark
                  ? "border-zinc-800 bg-zinc-900/90 text-zinc-400"
                  : "border-zinc-300 bg-zinc-100 text-zinc-600"
              }`}
            >
              {clip.source_site}
            </span>
            {clip.source_app && (
              <span className="text-[11px] text-zinc-500">via {clip.source_app}</span>
            )}
          </>
        )}
      </span>
    );
  }

  return (
    <span className="flex min-w-0 items-center gap-1.5">
      {clip.source_icon_path ? (
        <img
          src={convertFileSrc(clip.source_icon_path)}
          alt=""
          className="h-4 w-4 shrink-0 rounded-sm object-contain"
        />
      ) : (
        <svg
          width="14"
          height="14"
          viewBox="0 0 24 24"
          fill="none"
          className="shrink-0 text-zinc-500"
          aria-hidden="true"
        >
          <rect x="3" y="4" width="18" height="16" rx="2" stroke="currentColor" strokeWidth="1.8" />
          <path d="M3 9h18" stroke="currentColor" strokeWidth="1.8" />
        </svg>
      )}
      <span className="truncate font-medium">{clip.source_app ?? "Unknown app"}</span>
    </span>
  );
}