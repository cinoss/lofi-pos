import { Link } from "react-router-dom";
import type { ReactNode } from "react";

export interface Crumb {
  /** Display label. ReactNode so callers can drop in a `<Trans>` macro. */
  label: ReactNode;
  /** Internal route to link to. Omit for the current page (last crumb). */
  to?: string;
}

/**
 * Breadcrumb trail. Render at the top of detail-style routes so the staffer
 * can see and navigate back up the tree without relying on the browser back
 * button (Tauri webview's gesture support varies by platform).
 *
 *   <Breadcrumbs items={[
 *     { label: <Trans>Sessions</Trans>, to: "/sessions" },
 *     { label: spot.name },
 *   ]} />
 *
 * The last item is always rendered as plain text (no `to` needed); earlier
 * items render as links with a `›` separator between them.
 */
export function Breadcrumbs({ items }: { items: Crumb[] }) {
  return (
    <nav aria-label="Breadcrumb" className="mb-3 text-sm text-gray-500">
      <ol className="flex flex-wrap items-center gap-1">
        {items.map((c, i) => {
          const isLast = i === items.length - 1;
          return (
            <li key={i} className="flex items-center gap-1">
              {c.to && !isLast ? (
                <Link
                  to={c.to}
                  className="hover:underline text-blue-600"
                >
                  {c.label}
                </Link>
              ) : (
                <span className={isLast ? "text-gray-700 font-medium" : ""}>
                  {c.label}
                </span>
              )}
              {!isLast && <span aria-hidden="true">›</span>}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}
