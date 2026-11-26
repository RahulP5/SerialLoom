import { createElement, Fragment, type ReactNode } from "react";
import { create } from "zustand";
import { en, type Message, type MessageKey, type Messages } from "./en";

export type { MessageKey } from "./en";

/** Languages in the picker, each named in its own language. */
export const LANGUAGES = [
  { code: "en", name: "English" },
  { code: "de", name: "Deutsch" },
  { code: "es", name: "Español" },
  { code: "fr", name: "Français" },
  { code: "pt-BR", name: "Português (Brasil)" },
  { code: "ru", name: "Русский" },
  { code: "zh-CN", name: "简体中文" },
  { code: "ja", name: "日本語" },
  { code: "ko", name: "한국어" },
] as const;

export type Lang = (typeof LANGUAGES)[number]["code"];
export type LanguagePref = "system" | Lang;

/** Other languages load when first chosen, so the app does not carry all of them. */
const LOADERS: Record<Exclude<Lang, "en">, () => Promise<Messages>> = {
  de: () => import("./de").then((m) => m.de),
  es: () => import("./es").then((m) => m.es),
  fr: () => import("./fr").then((m) => m.fr),
  "pt-BR": () => import("./pt-BR").then((m) => m.ptBR),
  ru: () => import("./ru").then((m) => m.ru),
  "zh-CN": () => import("./zh-CN").then((m) => m.zhCN),
  ja: () => import("./ja").then((m) => m.ja),
  ko: () => import("./ko").then((m) => m.ko),
};

const MESSAGES: Partial<Record<Lang, Messages>> = { en };

/** Loads a language's messages; English when they cannot be loaded. */
async function ensure(lang: Lang): Promise<Lang> {
  if (MESSAGES[lang]) return lang;
  try {
    MESSAGES[lang] = await LOADERS[lang as Exclude<Lang, "en">]();
    return lang;
  } catch {
    return "en";
  }
}

const KEY = "serialloom.language";

function isLang(v: unknown): v is Lang {
  return LANGUAGES.some((l) => l.code === v);
}

/** The supported language closest to a BCP 47 tag, or null. */
function match(tag: string): Lang | null {
  const t = tag.toLowerCase();
  // Traditional Chinese readers are better served by English than by Simplified.
  if (t.startsWith("zh")) return /hant|-tw|-hk|-mo/.test(t) ? null : "zh-CN";
  if (t.startsWith("pt")) return "pt-BR";
  const base = t.split("-")[0];
  return LANGUAGES.find((l) => l.code === base)?.code ?? null;
}

/** The Windows display language, when SerialLoom has it; English otherwise. */
export function systemLanguage(): Lang {
  const tags = navigator.languages?.length ? navigator.languages : [navigator.language];
  for (const tag of tags) {
    const lang = tag && match(tag);
    if (lang) return lang;
  }
  return "en";
}

function load(): LanguagePref {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "system" || isLang(v)) return v;
  } catch {
    // Storage unavailable: use the default.
  }
  return "system";
}

function resolve(pref: LanguagePref): Lang {
  return pref === "system" ? systemLanguage() : pref;
}

interface I18nState {
  pref: LanguagePref;
  /** The language in effect; its messages are loaded. */
  lang: Lang;
  setPref(p: LanguagePref): Promise<void>;
}

function apply(lang: Lang) {
  // Fonts and line breaking follow the page language.
  document.documentElement.lang = lang;
  useI18n.setState({ lang });
}

export const useI18n = create<I18nState>((set, get) => ({
  pref: load(),
  lang: "en",
  async setPref(pref) {
    try {
      localStorage.setItem(KEY, pref);
    } catch {
      // Not remembered, but still applied for this session.
    }
    set({ pref });
    const lang = await ensure(resolve(pref));
    // A quicker later choice wins.
    if (get().pref === pref) apply(lang);
  },
}));

/** Loads the saved language. Call before the first render. */
export async function initI18n() {
  apply(await ensure(resolve(useI18n.getState().pref)));
}

const pluralRules = new Map<Lang, Intl.PluralRules>();

function pick(message: Message, lang: Lang, count: number): string {
  if (typeof message === "string") return message;
  let rules = pluralRules.get(lang);
  if (!rules) {
    rules = new Intl.PluralRules(lang);
    pluralRules.set(lang, rules);
  }
  return message[rules.select(count)] ?? message.other;
}

/** The raw message, for code that fills it in elsewhere (the tray menu). */
export function message(key: MessageKey): Message {
  return MESSAGES[useI18n.getState().lang]?.[key] ?? en[key];
}

type Vars = Record<string, string | number>;

/**
 * The message in the current language, with `{name}` filled in from `vars`.
 * A plural message picks its form by `vars.count`.
 */
export function t(key: MessageKey, vars?: Vars): string {
  const lang = useI18n.getState().lang;
  const text = pick(MESSAGES[lang]?.[key] ?? en[key], lang, Number(vars?.count ?? 0));
  if (!vars) return text;
  return text.replace(/\{(\w+)\}/g, (all, name: string) => (name in vars ? String(vars[name]) : all));
}

/** Like `t`, but `{name}` can be an element, such as a link or bold text. */
export function tr(key: MessageKey, parts: Record<string, ReactNode>): ReactNode {
  return t(key)
    .split(/\{(\w+)\}/)
    .map((piece, i) => createElement(Fragment, { key: i }, i % 2 ? (parts[piece] ?? `{${piece}}`) : piece));
}

/** `t` for components: re-renders them when the language changes. */
export function useT() {
  useI18n((s) => s.lang);
  return t;
}
