// Cron schedules as the simple choices the schedule picker offers, and back.
// Anything the picker can't express stays as a custom cron expression.

export type Frequency = "hours" | "daily" | "weekly" | "monthly" | "custom";

export interface ScheduleChoice {
  frequency: Frequency;
  /** "hours": run every N hours (1–12). */
  everyHours: number;
  /** "HH:MM", 24-hour. For "hours" only the minute is used. */
  time: string;
  /** 0 = Sunday. */
  weekday: number;
  /** 1–28, so every month has it. */
  dayOfMonth: number;
  /** "custom": the expression as typed. */
  cron: string;
}

export const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
export const HOUR_STEPS = [1, 2, 3, 4, 6, 8, 12];

const pad = (n: number) => String(n).padStart(2, "0");

function hm(time: string): [number, number] {
  const [h, m] = time.split(":").map((v) => Number.parseInt(v, 10));
  return [Number.isFinite(h) ? Math.min(Math.max(h, 0), 23) : 0, Number.isFinite(m) ? Math.min(Math.max(m, 0), 59) : 0];
}

export function toCron(c: ScheduleChoice): string {
  const [h, m] = hm(c.time);
  switch (c.frequency) {
    case "hours":
      return c.everyHours <= 1 ? `${m} * * * *` : `${m} */${c.everyHours} * * *`;
    case "daily":
      return `${m} ${h} * * *`;
    case "weekly":
      return `${m} ${h} * * ${c.weekday}`;
    case "monthly":
      return `${m} ${h} ${c.dayOfMonth} * *`;
    case "custom":
      return c.cron.trim();
  }
}

const num = (s: string, max: number) => (/^\d+$/.test(s) && Number(s) <= max ? Number(s) : null);

/** The picker's view of a cron expression; "custom" when it doesn't fit a preset. */
export function fromCron(cron: string): ScheduleChoice {
  const base: ScheduleChoice = { frequency: "custom", everyHours: 6, time: "02:00", weekday: 0, dayOfMonth: 1, cron: cron.trim() };
  const macro: Record<string, string> = { "@daily": "0 0 * * *", "@midnight": "0 0 * * *", "@hourly": "0 * * * *", "@weekly": "0 0 * * 0", "@monthly": "0 0 1 * *" };
  const f = (macro[cron.trim()] ?? cron).trim().split(/\s+/);
  if (f.length !== 5) return base;
  const [mi, ho, dom, mon, dow] = f;
  const m = num(mi, 59);
  if (m == null || mon !== "*") return base;
  const step = ho === "*" ? 1 : /^\*\/\d+$/.test(ho) ? Number(ho.slice(2)) : null;
  if (step != null && HOUR_STEPS.includes(step) && dom === "*" && dow === "*") {
    return { ...base, frequency: "hours", everyHours: step, time: `00:${pad(m)}` };
  }
  const h = num(ho, 23);
  if (h == null) return base;
  const time = `${pad(h)}:${pad(m)}`;
  if (dom === "*" && dow === "*") return { ...base, frequency: "daily", time };
  const wd = num(dow, 7);
  if (dom === "*" && wd != null) return { ...base, frequency: "weekly", time, weekday: wd % 7 };
  const d = num(dom, 28);
  if (d != null && d >= 1 && dow === "*") return { ...base, frequency: "monthly", time, dayOfMonth: d };
  return base;
}

function ordinal(n: number): string {
  const s = n % 100 >= 11 && n % 100 <= 13 ? "th" : ["th", "st", "nd", "rd"][n % 10] ?? "th";
  return `${n}${s}`;
}

/** "Every day at 02:00", "Every 6 hours at :15", … or the expression itself. */
export function describeCron(cron: string): string {
  const c = fromCron(cron);
  switch (c.frequency) {
    case "hours":
      return c.everyHours === 1 ? `Every hour at :${c.time.slice(3)}` : `Every ${c.everyHours} hours at :${c.time.slice(3)}`;
    case "daily":
      return `Every day at ${c.time}`;
    case "weekly":
      return `Every ${WEEKDAYS[c.weekday]} at ${c.time}`;
    case "monthly":
      return `On the ${ordinal(c.dayOfMonth)} of each month at ${c.time}`;
    case "custom":
      return cron.trim();
  }
}

/** Roughly how often it runs, in hours (custom expressions count as daily). */
export function intervalHours(cron: string): number {
  const c = fromCron(cron);
  switch (c.frequency) {
    case "hours":
      return c.everyHours;
    case "weekly":
      return 7 * 24;
    case "monthly":
      return 31 * 24;
    default:
      return 24;
  }
}

/** When a missing backup counts as overdue: one interval plus some slack. */
export function defaultMaxAgeHours(cron: string): number {
  const h = intervalHours(cron);
  return h + Math.max(2, Math.min(12, Math.round(h / 2)));
}

/** "about 2 weeks" for `keep` runs of `cron`. */
export function coverage(cron: string, keep: number): string {
  const hours = intervalHours(cron) * keep;
  if (hours < 48) return `about ${hours} hours`;
  const days = Math.round(hours / 24);
  if (days < 14) return `about ${days} days`;
  if (days < 62) return `about ${Math.round(days / 7)} weeks`;
  return `about ${Math.round(days / 30)} months`;
}

/** The PC's IANA time zone, e.g. "Europe/London". */
export function localTimeZone(): string | null {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || null;
  } catch {
    return null;
  }
}

export function timeZones(): string[] {
  const intl = Intl as unknown as { supportedValuesOf?: (key: string) => string[] };
  try {
    return intl.supportedValuesOf?.("timeZone") ?? [];
  } catch {
    return [];
  }
}
