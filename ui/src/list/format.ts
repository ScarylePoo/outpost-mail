const time = new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" });
const dayMonth = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short" });
const full = new Intl.DateTimeFormat(undefined, { day: "numeric", month: "short", year: "numeric" });
const long = new Intl.DateTimeFormat(undefined, { dateStyle: "full", timeStyle: "short" });

/** Short date for a list row: a time today, a day this year, a full date before that. */
export function formatDate(unixSeconds: number, now = new Date()): string {
  const d = new Date(unixSeconds * 1000);
  if (d.getFullYear() !== now.getFullYear()) return full.format(d);
  if (d.getMonth() === now.getMonth() && d.getDate() === now.getDate()) return time.format(d);
  return dayMonth.format(d);
}

export function formatLongDate(unixSeconds: number): string {
  return long.format(new Date(unixSeconds * 1000));
}
