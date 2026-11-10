- `gwi gmail sync` and `sync-all` no longer give up on a message after a few seconds of
  Gmail "units per minute per user" quota errors. The shared retry driver now makes up to
  six retries and waits up to 90 s in total (2, 4, 8, 16, 30, 30 s), long enough for a
  one-minute quota window to roll over; Drive requests share the same schedule.
