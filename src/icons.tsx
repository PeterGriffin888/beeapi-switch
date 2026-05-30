import { ToolId } from "./tools";

/**
 * Geometric SVG marks for each supported CLI. Monochrome, drawn on a
 * 24x24 viewBox, so they adapt to both light and dark theme via
 * `currentColor` for stroke and fill.
 */

export function ToolIcon({
  id,
  size = 22,
  className,
}: {
  id: ToolId;
  size?: number;
  className?: string;
}) {
  const common = {
    width: size,
    height: size,
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.6,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    className,
    "aria-hidden": true,
  };

  switch (id) {
    case "claude-code":
      // Stylised sunburst (Claude vibe): 8 rays + center dot.
      return (
        <svg {...common}>
          {Array.from({ length: 8 }).map((_, i) => {
            const a = (i * Math.PI) / 4;
            const x1 = 12 + Math.cos(a) * 4;
            const y1 = 12 + Math.sin(a) * 4;
            const x2 = 12 + Math.cos(a) * 9;
            const y2 = 12 + Math.sin(a) * 9;
            return (
              <line
                key={i}
                x1={x1.toFixed(2)}
                y1={y1.toFixed(2)}
                x2={x2.toFixed(2)}
                y2={y2.toFixed(2)}
              />
            );
          })}
          <circle cx="12" cy="12" r="2.6" fill="currentColor" />
        </svg>
      );

    case "codex":
      // Terminal bracket with a caret — cli-ish.
      return (
        <svg {...common}>
          <rect x="3" y="5" width="18" height="14" rx="2" />
          <path d="M7 10 L10 12 L7 14" />
          <line x1="12" y1="14.5" x2="17" y2="14.5" />
        </svg>
      );

    case "gemini-cli":
      // Four-point starburst (Gemini mark).
      return (
        <svg {...common}>
          <path
            d="M12 3 Q13.5 10.5 21 12 Q13.5 13.5 12 21 Q10.5 13.5 3 12 Q10.5 10.5 12 3 Z"
            fill="currentColor"
            stroke="none"
          />
        </svg>
      );

    case "opencode":
      // Chevron + underscore, "open code" suggestion.
      return (
        <svg {...common}>
          <path d="M8.5 8 L4.5 12 L8.5 16" />
          <path d="M15.5 8 L19.5 12 L15.5 16" />
          <line x1="10" y1="18" x2="14" y2="18" />
        </svg>
      );

    case "openclaw":
      // Claw: three descending arcs.
      return (
        <svg {...common}>
          <path d="M6 6 Q8 14 10 18" />
          <path d="M12 4 Q13 13 14 19" />
          <path d="M18 6 Q16 14 14 18" />
          <circle cx="12" cy="3.5" r="1.1" fill="currentColor" stroke="none" />
        </svg>
      );

    case "hermes":
      // Winged messenger: compact wing + command dot.
      return (
        <svg {...common}>
          <path d="M4 13 C7 6 12 5 20 6 C15 8 12 11 10 17" />
          <path d="M7 14 C10 10 13 9 18 10" />
          <path d="M10 17 L14 17" />
          <circle cx="7" cy="18" r="1.2" fill="currentColor" stroke="none" />
        </svg>
      );
  }
}

/** Icon for the "Tools" tab in the titlebar. */
export function ToolsTabIcon({ size = 13 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <rect x="3" y="4" width="8" height="7" rx="1.5" />
      <rect x="13" y="4" width="8" height="7" rx="1.5" />
      <rect x="3" y="13" width="8" height="7" rx="1.5" />
      <rect x="13" y="13" width="8" height="7" rx="1.5" />
    </svg>
  );
}

/** Key ring icon. */
export function KeysTabIcon({ size = 13 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <circle cx="8" cy="14" r="4" />
      <path d="M11 14 L20 14 L20 17 M17 14 L17 17" />
    </svg>
  );
}

/** Pulse / sessions icon. */
export function SessionsTabIcon({ size = 13 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M3 12 H7 L9 6 L13 18 L15 12 H21" />
    </svg>
  );
}

/** Trend / usage icon. */
export function UsageTabIcon({ size = 13 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M4 19 H20" />
      <rect x="6" y="13" width="2.5" height="6" rx="1" />
      <rect x="11" y="9" width="2.5" height="10" rx="1" />
      <rect x="16" y="5" width="2.5" height="14" rx="1" />
      <path d="M6 11 L11 8 L16 7 L19 4" />
      <circle cx="19" cy="4" r="1.2" fill="currentColor" stroke="none" />
    </svg>
  );
}

/** Playground / chat icon. */
export function PlaygroundTabIcon({ size = 13 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
      <circle cx="9" cy="10" r="1" fill="currentColor" stroke="none" />
      <circle cx="15" cy="10" r="1" fill="currentColor" stroke="none" />
    </svg>
  );
}

/** Sun / moon icon swapped by theme. */
export function ThemeIcon({
  theme,
  size = 14,
}: {
  theme: "light" | "dark";
  size?: number;
}) {
  if (theme === "light") {
    return (
      <svg
        width={size}
        height={size}
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
        aria-hidden
      >
        <circle cx="12" cy="12" r="4" />
        <path d="M12 3 V5 M12 19 V21 M3 12 H5 M19 12 H21 M5.6 5.6 L7 7 M17 17 L18.4 18.4 M5.6 18.4 L7 17 M17 7 L18.4 5.6" />
      </svg>
    );
  }
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <path d="M20 14.5 A8 8 0 1 1 9.5 4 A7 7 0 0 0 20 14.5 Z" />
    </svg>
  );
}

/** Info circle icon for the About tab. */
export function AboutTabIcon({ size = 13 }: { size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <circle cx="12" cy="12" r="9" />
      <line x1="12" y1="16" x2="12" y2="12" />
      <line x1="12" y1="8" x2="12.01" y2="8" />
    </svg>
  );
}
