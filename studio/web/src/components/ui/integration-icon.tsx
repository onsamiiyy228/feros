"use client";

import {
  Activity01Icon,
  AlertCircleIcon,
  Calendar01Icon,
  Call02Icon,
  Chart01Icon,
  CheckmarkSquare01Icon,
  CloudIcon,
  ConnectIcon,
  CpuIcon,
  CreditCardIcon,
  DatabaseIcon,
  File02Icon,
  HeadphonesIcon,
  InternetIcon,
  Key01Icon,
  Layout01Icon,
  Link01Icon,
  Mail01Icon,
  Message01Icon,
  Notification01Icon,
  Search01Icon,
  Shield01Icon,
  ShoppingCart01Icon,
  TableIcon,
  TriangleIcon,
  WebhookIcon,
  ZapIcon,
} from "@hugeicons/core-free-icons";
import { HugeiconsIcon } from "@hugeicons/react";
import Image from "next/image";
import { useState } from "react";

import { brandLogos } from "@/lib/integrations/brand-logos";
import { cn } from "@/lib/utils";

interface IntegrationIconProps {
  name: string;
  iconHint: string;
  size?: string;
  brandSize?: string;
  className?: string;
  brandTileClassName?: string;
  brandImageClassName?: string;
}

function SemanticIntegrationIcon({
  iconHint,
  size = "size-5",
  className,
}: Pick<IntegrationIconProps, "iconHint" | "size" | "className">) {
  const cls = cn(size, className ?? "text-foreground");

  switch (iconHint) {
    case "message-circle":
    case "message-square":
      return <HugeiconsIcon icon={Message01Icon} className={cls} />;
    case "calendar":
      return <HugeiconsIcon icon={Calendar01Icon} className={cls} />;
    case "table":
    case "sheet":
      return <HugeiconsIcon icon={TableIcon} className={cls} />;
    case "database":
      return <HugeiconsIcon icon={DatabaseIcon} className={cls} />;
    case "webhook":
      return <HugeiconsIcon icon={WebhookIcon} className={cls} />;
    case "key":
      return <HugeiconsIcon icon={Key01Icon} className={cls} />;
    case "shield":
      return <HugeiconsIcon icon={Shield01Icon} className={cls} />;
    case "globe":
      return <HugeiconsIcon icon={InternetIcon} className={cls} />;
    case "bar-chart-2":
      return <HugeiconsIcon icon={Chart01Icon} className={cls} />;
    case "cloud":
      return <HugeiconsIcon icon={CloudIcon} className={cls} />;
    case "file-text":
      return <HugeiconsIcon icon={File02Icon} className={cls} />;
    case "link":
      return <HugeiconsIcon icon={Link01Icon} className={cls} />;
    case "layout":
      return <HugeiconsIcon icon={Layout01Icon} className={cls} />;
    case "bell":
      return <HugeiconsIcon icon={Notification01Icon} className={cls} />;
    case "mail":
      return <HugeiconsIcon icon={Mail01Icon} className={cls} />;
    case "zap":
      return <HugeiconsIcon icon={ZapIcon} className={cls} />;
    case "search":
      return <HugeiconsIcon icon={Search01Icon} className={cls} />;
    case "cpu":
      return <HugeiconsIcon icon={CpuIcon} className={cls} />;
    case "phone":
      return <HugeiconsIcon icon={Call02Icon} className={cls} />;
    case "credit-card":
      return <HugeiconsIcon icon={CreditCardIcon} className={cls} />;
    case "shopping-cart":
      return <HugeiconsIcon icon={ShoppingCart01Icon} className={cls} />;
    case "check-square":
      return <HugeiconsIcon icon={CheckmarkSquare01Icon} className={cls} />;
    case "activity":
      return <HugeiconsIcon icon={Activity01Icon} className={cls} />;
    case "alert-circle":
      return <HugeiconsIcon icon={AlertCircleIcon} className={cls} />;
    case "headphones":
      return <HugeiconsIcon icon={HeadphonesIcon} className={cls} />;
    case "triangle":
      return <HugeiconsIcon icon={TriangleIcon} className={cls} />;
    default:
      return <HugeiconsIcon icon={ConnectIcon} className={cls} />;
  }
}

export function IntegrationIcon({
  name,
  iconHint,
  size = "size-5",
  brandSize = "size-full",
  className,
  brandTileClassName,
  brandImageClassName,
}: IntegrationIconProps) {
  const [brandFailed, setBrandFailed] = useState(false);
  const brandLogo = brandLogos[name as keyof typeof brandLogos];

  if (!brandLogo || brandFailed) {
    return (
      <SemanticIntegrationIcon
        iconHint={iconHint}
        size={size}
        className={className}
      />
    );
  }

  return (
    <span
      className={cn(
        "relative inline-flex items-center justify-center rounded-md bg-white p-0.5 shadow-[inset_0_0_0_1px_rgba(15,23,42,0.08)]",
        brandSize,
        brandTileClassName,
      )}
    >
      <Image
        src={brandLogo.src}
        alt={brandLogo.alt}
        fill
        unoptimized
        sizes="40px"
        className={cn("object-contain p-0.5", brandImageClassName)}
        onError={() => setBrandFailed(true)}
      />
    </span>
  );
}
