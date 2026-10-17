-- migrations/0005_event_location_url.sql
-- F24: LOCATION y URL como campos de primera clase del VEVENT, desnormalizados
-- junto al resto de propiedades consultables (`summary`, `description`, …).
--
-- `NULL` significa "sin LOCATION/URL": el `.ics` de origen no traía la propiedad
-- (o su valor era la cadena vacía). Es la misma convención que `0004` fijó para
-- `dtstart_tzid`/`dtend_tzid`.
--
-- Las filas escritas antes de esta migración quedan con ambas columnas a `NULL`
-- aunque su `raw_payload` sí contenga `LOCATION:`/`URL:`; la desnormalización se
-- añade después y no reescribe el payload. Es un hecho conocido y aceptado: el
-- `.ics` guardado sigue siendo la fuente de verdad. (Idéntico al caso de `0004`.)
--
-- Ambas columnas son aditivas: `ALTER TABLE ... ADD COLUMN`, sin reescritura de
-- tabla ni migración de datos.
ALTER TABLE calendar_objects ADD COLUMN location TEXT;
ALTER TABLE calendar_objects ADD COLUMN url      TEXT;
