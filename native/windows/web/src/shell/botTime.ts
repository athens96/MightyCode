// MC-64: how the sidebar 봇 group words a bot's next scheduled run.

/** MC-64: a bot's next run as its row shows it: the time today, else the month-day and time; none when not set or unreadable. */
export function nextRunWords(at: string | undefined, now: Date = new Date()): string | undefined {
  if (!at) return undefined;
  const when = new Date(at);
  if (Number.isNaN(when.getTime())) return undefined;
  const two = (n: number) => String(n).padStart(2, '0');
  const time = two(when.getHours()) + ':' + two(when.getMinutes());
  const today = when.getFullYear() === now.getFullYear() && when.getMonth() === now.getMonth() && when.getDate() === now.getDate();
  return today ? time : two(when.getMonth() + 1) + '-' + two(when.getDate()) + ' ' + time;
}
