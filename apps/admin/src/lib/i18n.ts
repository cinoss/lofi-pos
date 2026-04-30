import { i18n } from "@lingui/core";
import { messages as en } from "../locales/en/messages";
import { messages as vi } from "../locales/vi/messages";

export type SupportedLocale = "en" | "vi";
export const DEFAULT_LOCALE: SupportedLocale = "vi";

i18n.load({
  en,
  // vi inherits from en for any not-yet-translated strings
  vi: { ...en, ...vi },
});
i18n.activate(DEFAULT_LOCALE);

/** Activate a new locale; safe to call repeatedly. */
export function setLocale(locale: SupportedLocale) {
  i18n.activate(locale);
}

/** Convert a BCP-47 tag (e.g. "vi-VN") to a supported locale. */
export function normalizeLocale(
  tag: string | null | undefined,
): SupportedLocale {
  if (!tag) return DEFAULT_LOCALE;
  const lang = tag.split(/[-_]/)[0]?.toLowerCase();
  return lang === "en" ? "en" : "vi";
}

export { i18n };
