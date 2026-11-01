// present/time (`#624 voice-page-modularity`): how the page says when — a day, a range of days, an age,
// a message's time. Presentation decisions with no DOM: the views draw what these say.

export { briefAge } from "./ages.ts";
export {
  WEEKDAY_TIME_DAYS,
  calendarDaysBefore,
  editedTitle,
  fullLocalAt,
  fullLocalTime,
  messageDate,
  messageTimeLabel,
  shortLocalTime,
  shortTimeFormat,
  stamp,
} from "./clock.ts";
export type { Timed } from "./clock.ts";
export {
  DAY_MONTHS,
  DAY_WEEKDAYS,
  dayClock,
  dayKeyOf,
  dayLabel,
  dayParts,
  dayRangeWords,
  dayTitle,
  dayWords,
  sameLocalDay,
  searchDayWords,
} from "./days.ts";
export type { DayClock } from "./days.ts";
