-- migrations/0004_timezone.sql
-- Timezone support: store the IANA TZID alongside the denormalised DTSTART/DTEND
-- instants so the SPA and DAV clients can reconstruct the original wall-clock
-- time and local zone. NULL means "no TZID supplied" (floating/UTC time).
ALTER TABLE calendar_objects ADD COLUMN dtstart_tzid TEXT;
ALTER TABLE calendar_objects ADD COLUMN dtend_tzid TEXT;
