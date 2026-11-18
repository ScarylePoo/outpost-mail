// Dates for the calendar and tasks.
//
// Two kinds of time come from the core. A timed event has moments (unix
// seconds), shown in this computer's time zone. An all-day event or a
// task's due day has days, sent as midnight UTC of the day so that they
// mean the same day everywhere; those are read with UTC, never local time.

export const DAY = 86_400;

/** A calendar day, as the user sees it. */
export interface Day {
  year: number;
  /** 1 to 12. */
  month: number;
  day: number;
}

export function dayOfLocal(date: Date): Day {
  return { year: date.getFullYear(), month: date.getMonth() + 1, day: date.getDate() };
}

/** The day a moment falls on in this computer's time zone. */
export function dayOfMoment(unix: number): Day {
  return dayOfLocal(new Date(unix * 1000));
}

/** The day an all-day value (midnight UTC) stands for. */
export function dayOfDate(unix: number): Day {
  const d = new Date(unix * 1000);
  return { year: d.getUTCFullYear(), month: d.getUTCMonth() + 1, day: d.getUTCDate() };
}

/** Midnight UTC of a day: how days are sent to the core. */
export function dateValue(day: Day): number {
  return Date.UTC(day.year, day.month - 1, day.day) / 1000;
}

/** The moment a day starts in this computer's time zone. */
export function startOfDay(day: Day): number {
  return new Date(day.year, day.month - 1, day.day).getTime() / 1000;
}

export function addDays(day: Day, n: number): Day {
  return dayOfDate(dateValue(day) + n * DAY);
}

export function sameDay(a: Day, b: Day): boolean {
  return a.year === b.year && a.month === b.month && a.day === b.day;
}

export function today(): Day {
  return dayOfLocal(new Date());
}

/** A day number that orders days. */
export function dayNumber(day: Day): number {
  return dateValue(day) / DAY;
}

/** `2026-11-20`, for a date field. */
export function toInput(day: Day): string {
  return `${String(day.year).padStart(4, "0")}-${String(day.month).padStart(2, "0")}-${String(day.day).padStart(2, "0")}`;
}

export function fromInput(text: string): Day | undefined {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text);
  return m ? { year: Number(m[1]), month: Number(m[2]), day: Number(m[3]) } : undefined;
}

/** `14:30`, for a time field, in this computer's time zone. */
export function timeInput(unix: number): string {
  const d = new Date(unix * 1000);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** The moment of a day and a `14:30` time in this computer's time zone. */
export function moment(day: Day, time: string): number {
  const [h, m] = time.split(":").map(Number);
  return new Date(day.year, day.month - 1, day.day, h || 0, m || 0).getTime() / 1000;
}

const timeFormat = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const monthFormat = new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric", timeZone: "UTC" });
const dayFormat = new Intl.DateTimeFormat(undefined, { weekday: "long", day: "numeric", month: "long", year: "numeric", timeZone: "UTC" });
const shortFormat = new Intl.DateTimeFormat(undefined, { weekday: "short", day: "numeric", month: "short", timeZone: "UTC" });
const weekdayFormat = new Intl.DateTimeFormat(undefined, { weekday: "short", timeZone: "UTC" });

export const formatTime = (unix: number) => timeFormat.format(new Date(unix * 1000));
export const formatMonth = (day: Day) => monthFormat.format(new Date(dateValue(day) * 1000));
export const formatDay = (day: Day) => dayFormat.format(new Date(dateValue(day) * 1000));
export const formatShort = (day: Day) => shortFormat.format(new Date(dateValue(day) * 1000));

/** The first day of the week here: Monday, except where the locale says Sunday. */
export function weekStartsOn(): number {
  try {
    const locale = new Intl.Locale(navigator.language) as Intl.Locale & { weekInfo?: { firstDay: number }; getWeekInfo?: () => { firstDay: number } };
    const first = (locale.getWeekInfo?.() ?? locale.weekInfo)?.firstDay;
    if (first !== undefined) return first % 7;
  } catch {
    // Not known: Monday.
  }
  return 1;
}

/** Short weekday names, starting from the week's first day. */
export function weekdayNames(): string[] {
  const first = weekStartsOn();
  // 4 January 1970 was a Sunday.
  return Array.from({ length: 7 }, (_, i) => weekdayFormat.format(new Date((3 + first + i) * DAY * 1000)));
}

/** The 42 days a month's grid shows: whole weeks around the month. */
export function monthGrid(year: number, month: number): Day[] {
  const first = { year, month, day: 1 };
  const weekday = new Date(dateValue(first) * 1000).getUTCDay();
  const lead = (weekday - weekStartsOn() + 7) % 7;
  return Array.from({ length: 42 }, (_, i) => addDays(first, i - lead));
}
