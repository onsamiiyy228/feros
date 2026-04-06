"use client";

import { useEffect, useState } from "react";
import { cn } from "@/lib/utils";
import { highlightCode, type ShikiLanguage } from "@/lib/shiki";

interface ShikiCodeBlockProps {
  code: string;
  lang: ShikiLanguage;
  className?: string;
}

export default function ShikiCodeBlock({ code, lang, className }: ShikiCodeBlockProps) {
  const [html, setHtml] = useState<string>("");
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let cancelled = false;

    void highlightCode(code, lang)
      .then((value) => {
        if (!cancelled) {
          setHtml(value);
          setFailed(false);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setFailed(true);
        }
      });

    return () => {
      cancelled = true;
    };
  }, [code, lang]);

  if (failed || !html) {
    return (
      <pre className={cn("m-0 overflow-auto p-6 text-[13px] leading-6 text-foreground", className)}>
        {code}
      </pre>
    );
  }

  return (
    <div
      className={cn(
        "overflow-auto bg-transparent",
        "[&_.shiki]:m-0!",
        "[&_.shiki]:bg-transparent!",
        "[&_.shiki]:leading-relaxed!",
        "[&_.shiki]:p-3",
        "[&_.shiki]:text-xs",
        "[&_.line]:relative",
        "[&_.line]:pl-10",
        "[&_.shiki-line-number]:absolute",
        "[&_.shiki-line-number]:left-0",
        "[&_.shiki-line-number]:w-10",
        "[&_.shiki-line-number]:pr-4",
        "[&_.shiki-line-number]:text-right",
        "[&_.shiki-line-number]:text-xs",
        "[&_.shiki-line-number]:text-muted-foreground/50",
        "[&_.shiki-line-number]:select-none",
        className
      )}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}
