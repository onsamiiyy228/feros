import { createHighlighterCore } from "@shikijs/core";
import { createJavaScriptRegexEngine } from "@shikijs/engine-javascript";
import json from "@shikijs/langs/json";
import javascript from "@shikijs/langs/javascript";
import typescript from "@shikijs/langs/typescript";
import theme from "@shikijs/themes/slack-ochin";

export type ShikiLanguage = "json" | "javascript" | "typescript";

const SHIKI_THEME = "slack-ochin";

let highlighterPromise: ReturnType<typeof createHighlighterCore> | null = null;

async function getHighlighter() {
  if (!highlighterPromise) {
    highlighterPromise = createHighlighterCore({
      engine: createJavaScriptRegexEngine(),
      langs: [json, javascript, typescript],
      themes: [theme],
    });
  }
  return highlighterPromise;
}

function addLineNumbers(html: string): string {
  let line = 0;
  return html.replace(/<span class="line">/g, () => {
    line += 1;
    return `<span class="line"><span class="shiki-line-number">${line}</span>`;
  });
}

export async function highlightCode(code: string, lang: ShikiLanguage): Promise<string> {
  const highlighter = await getHighlighter();
  const html = highlighter.codeToHtml(code, {
    lang,
    theme: SHIKI_THEME,
  });
  return addLineNumbers(html);
}
