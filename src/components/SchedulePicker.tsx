import { useEffect, useMemo, useState } from "react";
import { HOUR_STEPS, WEEKDAYS, fromCron, toCron, type Frequency, type ScheduleChoice } from "../lib/schedule";

const FREQUENCIES: { value: Frequency; label: string }[] = [
  { value: "hours", label: "Every few hours" },
  { value: "daily", label: "Daily" },
  { value: "weekly", label: "Weekly" },
  { value: "monthly", label: "Monthly" },
  { value: "custom", label: "Custom (cron)" },
];

/**
 * How often something runs, as plain choices (every N hours, daily, weekly,
 * monthly at a time) with a cron expression as the fallback. Controlled by
 * the cron string; an expression the presets can't show opens as Custom.
 */
export default function SchedulePicker({
  cron,
  onChange,
  allowCustom = true,
}: {
  cron: string;
  onChange: (cron: string) => void;
  /** Off for schedules this PC runs: they only understand the presets. */
  allowCustom?: boolean;
}) {
  // Kept locally so switching to Custom keeps the text being typed.
  const [choice, setChoice] = useState<ScheduleChoice>(() => fromCron(cron));
  // Follow outside changes (Discard), not our own.
  useEffect(() => {
    if (toCron(choice) !== cron.trim()) setChoice(fromCron(cron));
  }, [cron]); // eslint-disable-line react-hooks/exhaustive-deps
  const update = (patch: Partial<ScheduleChoice>) => {
    const next = { ...choice, ...patch };
    if (patch.frequency === "custom" && choice.frequency !== "custom") next.cron = toCron(choice);
    setChoice(next);
    onChange(toCron(next));
  };
  const minutes = useMemo(() => Array.from({ length: 12 }, (_, i) => String(i * 5).padStart(2, "0")), []);

  return (
    <div className="flex flex-wrap items-end gap-2">
      <label className="block">
        <span className="field-label">How often</span>
        <select className="field" value={choice.frequency} onChange={(e) => update({ frequency: e.target.value as Frequency })}>
          {FREQUENCIES.filter((f) => allowCustom || f.value !== "custom" || choice.frequency === "custom").map((f) => (
            <option key={f.value} value={f.value}>
              {f.label}
            </option>
          ))}
        </select>
      </label>
      {choice.frequency === "hours" && (
        <>
          <label className="block">
            <span className="field-label">Every</span>
            <select className="field" value={choice.everyHours} onChange={(e) => update({ everyHours: Number(e.target.value) })}>
              {HOUR_STEPS.map((h) => (
                <option key={h} value={h}>
                  {h === 1 ? "hour" : `${h} hours`}
                </option>
              ))}
            </select>
          </label>
          <label className="block">
            <span className="field-label">At minute</span>
            <select className="field" value={choice.time.slice(3)} onChange={(e) => update({ time: `00:${e.target.value}` })}>
              {minutes.includes(choice.time.slice(3)) ? null : <option value={choice.time.slice(3)}>:{choice.time.slice(3)}</option>}
              {minutes.map((m) => (
                <option key={m} value={m}>
                  :{m}
                </option>
              ))}
            </select>
          </label>
        </>
      )}
      {choice.frequency === "weekly" && (
        <label className="block">
          <span className="field-label">On</span>
          <select className="field" value={choice.weekday} onChange={(e) => update({ weekday: Number(e.target.value) })}>
            {WEEKDAYS.map((d, i) => (
              <option key={d} value={i}>
                {d}
              </option>
            ))}
          </select>
        </label>
      )}
      {choice.frequency === "monthly" && (
        <label className="block">
          <span className="field-label">On day</span>
          <select className="field" value={choice.dayOfMonth} onChange={(e) => update({ dayOfMonth: Number(e.target.value) })}>
            {Array.from({ length: 28 }, (_, i) => i + 1).map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
        </label>
      )}
      {(choice.frequency === "daily" || choice.frequency === "weekly" || choice.frequency === "monthly") && (
        <label className="block">
          <span className="field-label">At</span>
          <input className="field" type="time" value={choice.time} onChange={(e) => e.target.value && update({ time: e.target.value })} />
        </label>
      )}
      {choice.frequency === "custom" && (
        <label className="block min-w-48 flex-1">
          <span className="field-label">Cron expression (minute hour day month weekday)</span>
          <input className="field mono w-full" value={choice.cron} placeholder="0 2 * * *" onChange={(e) => update({ cron: e.target.value })} />
        </label>
      )}
    </div>
  );
}
