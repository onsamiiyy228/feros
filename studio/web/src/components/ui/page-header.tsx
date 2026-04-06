import { HugeiconsIcon, type IconSvgElement } from "@hugeicons/react";

import { cn } from "@/lib/utils";

interface PageHeaderProps {
  icon: IconSvgElement;
  title: string;
  description?: string;
  className?: string;
}

export function PageHeader({ icon, title, description, className }: PageHeaderProps) {
  const hasDescription = Boolean(description);

  return (
    <div className={cn("flex items-center gap-4", className)}>
      <div className="size-12 rounded-xl bg-primary/10 flex items-center justify-center shrink-0">
        <HugeiconsIcon icon={icon} className="size-6 text-primary" />
      </div>
      <div className="min-w-0">
        <h1
          className={cn(
            "font-semibold tracking-tight text-foreground",
            hasDescription ? "text-lg" : "text-xl",
          )}
        >
          {title}
        </h1>
        {hasDescription ? (
          <p className="text-xs text-muted-foreground mt-0.5">{description}</p>
        ) : null}
      </div>
    </div>
  );
}

