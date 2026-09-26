// Shared Intl.DateTimeFormat cache (formatters are costly to build and are asked for per render).

export interface ClockOptions {
  /** BCP 47 locale; defaults to the user's. */
  locale?: string;
  /** IANA zone; defaults to the system zone. */
  timeZone?: string;
}

const fmtCache = new Map<string, Intl.DateTimeFormat>();

/** A cached formatter for the locale and zone of `opts` with the given fields. */
export function dtf(opts: ClockOptions, fmt: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  const key = JSON.stringify([opts.locale ?? "", opts.timeZone ?? "", fmt]);
  let f = fmtCache.get(key);
  if (!f) {
    f = new Intl.DateTimeFormat(opts.locale, { ...fmt, timeZone: opts.timeZone });
    fmtCache.set(key, f);
  }
  return f;
}
