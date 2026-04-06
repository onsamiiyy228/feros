"use client";

import { HugeiconsIcon } from "@hugeicons/react";
import { ArrowLeft01Icon } from "@hugeicons/core-free-icons";
import Link from "next/link";
import { useParams } from "next/navigation";
import { useEffect, useMemo, useState } from "react";
import { CallHeader } from "@/components/call-detail/call-header";
import { LogTab } from "@/components/call-detail/log-tab";
import { CallWaveform } from "@/components/call-detail/call-waveform";
import { TranscriptTab } from "@/components/call-detail/transcript-tab";
import {
  extractTranscriptMessages,
  parseTranscript,
} from "@/components/call-detail/types";
import { Button } from "@/components/ui/button";
import {
  api,
  API_BASE,
  type CallEvent,
  type CallLog,
  type CallLogCapabilities,
} from "@/lib/api/client";

function getAbsoluteUrl(url: string | null | undefined): string | undefined {
  if (!url) return undefined;
  return url.startsWith('/') ? `${API_BASE}${url}` : url;
}

type TabKey = "transcript" | "log";

export default function CallDetailPage() {
  const params = useParams<{ id: string }>();
  const callId = params?.id;
  const hasValidId = typeof callId === "string" && callId.length > 0;

  const [error, setError] = useState<string | null>(null);
  const [call, setCall] = useState<CallLog | null>(null);
  const [activeTab, setActiveTab] = useState<TabKey>("transcript");
  const [currentTimeSec, setCurrentTimeSec] = useState(0);
  const [audioDurationSec, setAudioDurationSec] = useState(0);
  const [logCapabilities, setLogCapabilities] = useState<CallLogCapabilities | null>(null);
  const [events, setEvents] = useState<CallEvent[]>([]);
  const [eventsLoading, setEventsLoading] = useState(false);
  const [eventsError, setEventsError] = useState<string | null>(null);

  useEffect(() => {
    if (!hasValidId || !callId) {
      return;
    }

    let cancelled = false;

    api.calls
      .get(callId)
      .then((data) => {
        if (cancelled) return;
        setCall(data);
        setError(null);
      })
      .catch((err) => {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : "Failed to load call detail.");
        setCall(null);
      });

    api.calls
      .getLogCapabilities(callId)
      .then((data) => {
        if (cancelled) return;
        setLogCapabilities(data);
      })
      .catch(() => {
        if (cancelled) return;
        setLogCapabilities(null);
      });

    return () => {
      cancelled = true;
    };
  }, [callId, hasValidId]);

  useEffect(() => {
    if (!hasValidId || !callId || !logCapabilities?.has_internal_logs) return;

    let cancelled = false;
    setTimeout(() => {
      setEventsLoading(true);
      setEventsError(null);
    }, 0);
    api.calls
      .getEvents(callId, 0, 500)
      .then((data) => {
        if (cancelled) return;
        setEvents(data.events);
      })
      .catch((err) => {
        if (cancelled) return;
        setEventsError(err instanceof Error ? err.message : "Failed to load call events.");
        setEvents([]);
      })
      .finally(() => {
        if (cancelled) return;
        setEventsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [callId, hasValidId, logCapabilities?.has_internal_logs]);

  const transcriptDoc = useMemo(() => parseTranscript(call?.transcript_json), [call]);
  const transcriptMessages = useMemo(
    () => extractTranscriptMessages(transcriptDoc),
    [transcriptDoc]
  );

  const transcriptSyncScale = useMemo(() => {
    const audioDur = audioDurationSec;
    if (!(audioDur > 0)) return 1;

    const wallclockDur = transcriptDoc?.wallclockDurationSec;
    if (wallclockDur && wallclockDur > 0) {
      const scale = audioDur / wallclockDur;
      if (scale > 0.5 && scale < 1.5) return scale;
    }

    if (call?.duration_seconds && call.duration_seconds > 0) {
      const scale = audioDur / call.duration_seconds;
      if (scale > 0.5 && scale < 1.5) return scale;
    }

    return 1;
  }, [audioDurationSec, transcriptDoc?.wallclockDurationSec, call?.duration_seconds]);

  const transcriptLeadInSec = useMemo(() => {
    for (const msg of transcriptMessages) {
      if (msg.offsetSecRaw != null) {
        const shifted = msg.offsetSecRaw * transcriptSyncScale;
        return shifted > 0 ? shifted : 0;
      }
    }
    return 0;
  }, [transcriptMessages, transcriptSyncScale]);

  const activeTranscriptIndex = useMemo(() => {
    const alignedTimeSec = currentTimeSec + transcriptLeadInSec;
    let activeIndex = -1;
    for (let i = 0; i < transcriptMessages.length; i += 1) {
      const offsetRaw = transcriptMessages[i]?.offsetSecRaw;
      const offset = offsetRaw == null ? null : offsetRaw * transcriptSyncScale;
      if (offset == null) continue;
      if (offset <= alignedTimeSec) {
        activeIndex = i;
      }
    }
    return activeIndex;
  }, [transcriptMessages, currentTimeSec, transcriptSyncScale, transcriptLeadInSec]);

  const canShowLogTab =
    (logCapabilities?.has_internal_logs ?? false) ||
    (logCapabilities?.external_links?.length ?? 0) > 0;
  const langfuseLink = useMemo(() => {
    const active = (logCapabilities?.active_adapters ?? []).some(
      (name) => name.toLowerCase() === "langfuse"
    );
    if (!active) return null;
    return (
      (logCapabilities?.external_links ?? []).find(
        (link) => link.adapter.toLowerCase() === "langfuse"
      ) ?? null
    );
  }, [logCapabilities]);

  // Sync tab state if capabilities change
  useEffect(() => {
    if (activeTab === "log" && logCapabilities && !canShowLogTab) {
      setTimeout(() => setActiveTab("transcript"), 0);
    }
  }, [activeTab, canShowLogTab, logCapabilities]);

  if (!hasValidId) {
    return (
      <div className="space-y-4">
        <Link href="/dashboard/calls">
          <Button variant="ghost" size="sm" className="h-8 gap-1.5 text-xs">
            <HugeiconsIcon icon={ArrowLeft01Icon} className="size-3.5" />
            Back to Calls
          </Button>
        </Link>
        <div className="flat-card p-10 text-center">
          <p className="text-sm text-foreground font-medium">Unable to open call detail</p>
          <p className="text-sm text-muted-foreground mt-1">Invalid call ID.</p>
        </div>
      </div>
    );
  }

  const loading = !error && !call;

  if (loading) {
    return (
      <div className="space-y-4">
        <div className="h-24 rounded-xl bg-secondary/30 animate-pulse" />
        <div className="h-52 rounded-xl bg-secondary/30 animate-pulse" />
        <div className="h-64 rounded-xl bg-secondary/30 animate-pulse" />
      </div>
    );
  }

  if (error || !call) {
    return (
      <div className="space-y-4">
        <Link href="/dashboard/calls">
          <Button variant="ghost" size="sm" className="h-8 gap-1.5 text-xs">
            <HugeiconsIcon icon={ArrowLeft01Icon} className="size-3.5" />
            Back to Calls
          </Button>
        </Link>
        <div className="flat-card p-10 text-center">
          <p className="text-sm text-foreground font-medium">Unable to open call detail</p>
          <p className="text-sm text-muted-foreground mt-1">{error ?? "Call not found."}</p>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-5">
      <div className="flex items-center justify-between">
        <Link href="/dashboard/calls">
          <Button variant="ghost" size="sm" className="h-8 gap-1.5 text-xs">
            <HugeiconsIcon icon={ArrowLeft01Icon} className="size-3.5" />
            Back to Calls
          </Button>
        </Link>
      </div>

      <CallHeader call={call} />

      <CallWaveform
        key={call.recording_url ?? "no-recording"}
        recordingUrl={getAbsoluteUrl(call.recording_url) ?? null}
        fallbackDurationSec={call.duration_seconds}
        onDurationReady={setAudioDurationSec}
        onTimeUpdate={setCurrentTimeSec}
      />

      <section className="flat-card p-5 space-y-4">
        <div className="flex items-center justify-between gap-3">
          <div className="inline-flex rounded-lg border border-border/50 p-1 bg-secondary/30">
            <button
              type="button"
              className={`rounded-md px-3 py-1.5 text-xs font-medium transition-colors ${
                activeTab === "transcript"
                  ? "bg-card text-foreground shadow-sm"
                  : "text-muted-foreground hover:text-foreground"
              }`}
              onClick={() => setActiveTab("transcript")}
            >
              Transcript
            </button>
            {canShowLogTab ? (
              <button
                type="button"
                className={`rounded-md px-3 py-1.5 text-xs font-medium transition-colors ${
                  activeTab === "log"
                    ? "bg-card text-foreground shadow-sm"
                    : "text-muted-foreground hover:text-foreground"
                }`}
                onClick={() => setActiveTab("log")}
              >
                Log
              </button>
            ) : null}
          </div>

          {activeTab === "log" && langfuseLink ? (
            <a
              href={langfuseLink.url}
              target="_blank"
              rel="noreferrer"
              className="inline-flex items-center rounded-md border border-border/60 px-3 py-1.5 text-xs text-muted-foreground hover:bg-secondary/40 hover:text-foreground"
            >
              Also available in LangFuse
            </a>
          ) : null}
        </div>

        {activeTab === "transcript" ? (
          <TranscriptTab
            messages={transcriptMessages}
            activeIndex={activeTranscriptIndex}
          />
        ) : (
          <LogTab
            hasInternalLogs={logCapabilities?.has_internal_logs ?? false}
            events={events}
            eventsLoading={eventsLoading}
            eventsError={eventsError}
            externalLinks={logCapabilities?.external_links ?? []}
            totalDurationSec={call.duration_seconds}
          />
        )}
      </section>
    </div>
  );
}
