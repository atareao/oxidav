/**
 * Vista Calendario (F11).
 *
 * Decisión de rejilla: se implementa una rejilla mensual propia (6×7) sobre los
 * helpers de `src/utils/date.ts` en lugar de una librería externa
 * (`@schedule-x/react`, FullCalendar, …). Motivo: varias librerías de calendario
 * todavía no declaran soporte estable para React 19 y arrastran dependencias
 * pesadas; la rejilla de solo lectura + modales de Ant Design cubre el alcance
 * de F11 (listar/crear/editar/borrar con estado optimista) sin riesgo de
 * incompatibilidad. Si más adelante hacen falta vistas de semana/hora o
 * drag&drop, se reevaluará una librería compatible con React 19.
 *
 * Actualizaciones optimistas: React 19 `useOptimistic` sobre la lista base
 * confirmada por el servidor. Cada mutación se ejecuta dentro de un
 * `startTransition`; al terminar la transición la capa optimista se reconcilia
 * automáticamente, por lo que un fallo de la API revierte la UI al estado previo
 * (y el error se muestra con `message`/`Alert`).
 */

import {
  Alert,
  App,
  Button,
  Card,
  DatePicker,
  Empty,
  Flex,
  Form,
  Input,
  Modal,
  Popconfirm,
  Select,
  Space,
  Typography,
} from 'antd'
import dayjs from 'dayjs'
import type { Dayjs } from 'dayjs'
import {
  startTransition,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useOptimistic,
} from 'react'
import { useEvents } from '../hooks/useEvents'
import { toErrorMessage } from '../services/api'
import { listCalendarCollections } from '../services/collections'
import { createEvent, deleteEvent, updateEvent } from '../services/events'
import type { CalendarEvent, Collection, CreateEventInput, UpdateEventInput } from '../types/api'
import {
  addMonths,
  buildMonthGrid,
  dateKey,
  endOfMonthMs,
  formatDayLabel,
  formatMonthLabel,
  isSameMonth,
  startOfMonthMs,
} from '../utils/date'
import { tempId } from '../utils/tempId'
import { PAGE_GAP, fluidControlStyle } from './pageLayout'

const { RangePicker } = DatePicker

/** Column headers of the month grid (Monday-first, matching `buildMonthGrid`). */
const WEEKDAYS = ['Lun', 'Mar', 'Mié', 'Jue', 'Vie', 'Sáb', 'Dom'] as const

const timeFormatter = new Intl.DateTimeFormat('es-ES', { hour: '2-digit', minute: '2-digit' })

function formatTime(ms: number | null): string {
  return ms === null ? 'Todo el día' : timeFormatter.format(new Date(ms))
}

/**
 * The browser's IANA time zone, used to anchor created/edited events (F18) so a
 * recurrence repeats in local wall-clock time instead of drifting across a DST
 * change. Returns `undefined` when the runtime cannot resolve it, in which case
 * the field must be **omitted** — the API rejects an empty `tzid` with a 400.
 */
function browserTimeZone(): string | undefined {
  const zone = Intl.DateTimeFormat().resolvedOptions().timeZone
  return zone === undefined || zone === '' ? undefined : zone
}

function defaultRange(): [Dayjs, Dayjs] {
  const start = dayjs().second(0).millisecond(0)
  return [start, start.add(1, 'hour')]
}

/** Shape of the `Form` values (the range covers `dtstart`/`dtend`). */
interface EventFormValues {
  summary: string
  description?: string
  location?: string
  url?: string
  range: [Dayjs, Dayjs]
}

/** Fields the optimistic layer may rewrite on an existing event. */
type EventChanges = Pick<
  CalendarEvent,
  'summary' | 'description' | 'location' | 'url' | 'dtstart' | 'dtend' | 'dtstart_tzid' | 'dtend_tzid'
>

/**
 * Is `value` safe to render as a clickable link? The server already allowlists
 * `http://`/`https://` (F24), but defence in depth: the browser must not be the
 * last filter, so anything else stays plain text.
 */
function isSafeHttpUrl(value: string | null): value is string {
  if (value === null) {
    return false
  }
  const lower = value.toLowerCase()
  return lower.startsWith('http://') || lower.startsWith('https://')
}

/**
 * Mirror of the server's URL frontier (F24), used to give immediate feedback
 * instead of a `400` on save.
 *
 * The server validates **only** a value supplied by the request and lets an
 * inherited one (a `URL:mailto:…`/`ftp://…` an imported `.ics` already carries)
 * pass through verbatim, so a `PATCH` can rename such an event without wiping
 * its link (see `src/api/events.rs`). This validator reproduces that contract:
 *
 * - a value equal to the one the dialog opened with (`baseline`, already
 *   trimmed) is accepted untouched, whatever its scheme;
 * - a **changed** value must be an absolute `http://`/`https://` URL free of
 *   control characters and whitespace.
 *
 * The value is scrutinised trimmed — exactly how it is compared and sent — so a
 * pasted `"https://ejemplo.org/x "` is accepted and stored without the trailing
 * space instead of reaching the server and being rejected.
 *
 * Returns `null` when valid, otherwise the error message.
 */
function validateUrlValue(value: unknown, baseline: string): string | null {
  if (typeof value !== 'string') {
    return null
  }
  const trimmed = value.trim()
  if (trimmed === '' || trimmed === baseline) {
    return null
  }
  // `URL` is emitted verbatim into the `.ics`, so a control character or an
  // (inner) whitespace could inject a content line — same rule as the server.
  // `\p{Cc}` covers the C0/C1 control blocks without spelling out their ranges.
  if (/[\s\p{Cc}]/u.test(trimmed)) {
    return 'La URL no puede contener espacios ni caracteres de control'
  }
  const lower = trimmed.toLowerCase()
  const rest = lower.startsWith('http://')
    ? lower.slice('http://'.length)
    : lower.startsWith('https://')
      ? lower.slice('https://'.length)
      : null
  if (rest === null || rest === '') {
    return 'La URL debe empezar por http:// o https://'
  }
  return null
}

type OptimisticAction =
  | { kind: 'create'; event: CalendarEvent }
  | { kind: 'update'; uid: string; changes: EventChanges }
  | { kind: 'delete'; uid: string }

/** Pure reducer applied by `useOptimistic` on top of the server-confirmed list. */
function applyOptimistic(events: CalendarEvent[], action: OptimisticAction): CalendarEvent[] {
  switch (action.kind) {
    case 'create':
      return [...events, action.event]
    case 'update':
      return events.map((event) =>
        event.uid === action.uid ? { ...event, ...action.changes } : event,
      )
    case 'delete':
      return events.filter((event) => event.uid !== action.uid)
  }
}

export function Calendar() {
  // Context-aware message API: unlike the static `message` export it inherits
  // the active Ant Design theme (light/dark) from the closest `<App>` provider.
  const { message } = App.useApp()

  const [month, setMonth] = useState<Date>(() => new Date())
  const [selectedCollectionId, setSelectedCollectionId] = useState<string | null>(null)

  const [collections, setCollections] = useState<Collection[]>([])
  const [collectionsError, setCollectionsError] = useState<string | null>(null)
  const [collectionsKey, setCollectionsKey] = useState<string | null>(null)
  const [collectionsReloadToken, setCollectionsReloadToken] = useState(0)

  const [modalOpen, setModalOpen] = useState(false)
  const [editing, setEditing] = useState<CalendarEvent | null>(null)
  const [mutationError, setMutationError] = useState<string | null>(null)
  // `saving` drives the Modal's `confirmLoading` (disables OK); the ref is the
  // synchronous guard that drops the *second* of two submits fired in the same
  // tick, before React has re-rendered the disabled button.
  const [saving, setSaving] = useState(false)
  const savingRef = useRef(false)

  const [form] = Form.useForm<EventFormValues>()

  // Load (only) the calendar collections. Loading state is *derived* from a
  // request key instead of being written synchronously inside the effect, which
  // the `react-hooks/set-state-in-effect` rule forbids.
  const collectionsRequestKey = `calendar-collections#${collectionsReloadToken}`
  useEffect(() => {
    const controller = new AbortController()
    listCalendarCollections(controller.signal)
      .then((items) => {
        // Stale-response guard: a newer request (or unmount) aborted us while
        // this response was in flight, so it must not overwrite fresher state.
        if (controller.signal.aborted) {
          return
        }
        setCollections(items)
        setCollectionsError(null)
        setCollectionsKey(collectionsRequestKey)
      })
      .catch((error: unknown) => {
        if (!controller.signal.aborted) {
          setCollectionsError(toErrorMessage(error))
          setCollectionsKey(collectionsRequestKey)
        }
      })
    return () => {
      controller.abort()
    }
  }, [collectionsRequestKey])

  const collectionsLoading = collectionsKey !== collectionsRequestKey

  // Derive the selected collection instead of storing it via an effect: if the
  // stored id is gone (or none was picked yet), fall back to the first one.
  const effectiveCollectionId = useMemo(() => {
    if (selectedCollectionId !== null && collections.some((item) => item.id === selectedCollectionId)) {
      return selectedCollectionId
    }
    return collections[0]?.id ?? null
  }, [selectedCollectionId, collections])

  // Memoised so the `useEvents` effect does not re-run on every render.
  const range = useMemo(
    () => ({ start: startOfMonthMs(month), end: endOfMonthMs(month) }),
    [month],
  )

  const {
    events,
    setEvents,
    loading: eventsLoading,
    error: eventsError,
    reload: reloadEvents,
  } = useEvents(effectiveCollectionId, range)

  const [optimisticEvents, addOptimistic] = useOptimistic(events, applyOptimistic)

  const days = useMemo(() => buildMonthGrid(month), [month])

  const eventsByDay = useMemo(() => {
    const grouped = new Map<string, CalendarEvent[]>()
    for (const event of optimisticEvents) {
      if (event.dtstart === null) {
        continue
      }
      const key = dateKey(new Date(event.dtstart))
      const bucket = grouped.get(key)
      if (bucket === undefined) {
        grouped.set(key, [event])
      } else {
        bucket.push(event)
      }
    }
    for (const bucket of grouped.values()) {
      bucket.sort((a, b) => (a.dtstart ?? 0) - (b.dtstart ?? 0))
    }
    return grouped
  }, [optimisticEvents])

  const openCreate = useCallback(() => {
    setMutationError(null)
    setEditing(null)
    setModalOpen(true)
  }, [])

  const openEdit = useCallback((event: CalendarEvent) => {
    setMutationError(null)
    setEditing(event)
    setModalOpen(true)
  }, [])

  const handleSubmit = useCallback(
    (values: EventFormValues) => {
      // Drop submits while one is already in flight (double-click / Enter spam).
      if (savingRef.current) {
        return
      }
      const collectionId = effectiveCollectionId
      if (collectionId === null) {
        return
      }
      const [start, end] = values.range
      const dtstart = start.valueOf()
      const dtend = end.valueOf()
      // Raw form values, kept for the "did the user change it?" comparison (D2);
      // `description` keeps its pre-existing trim when it *is* sent.
      const rawDescription = values.description ?? ''
      const description = rawDescription.trim()
      const hasDescription = description.length > 0
      // `location` and `url` are trimmed exactly like `description`: an accidental
      // trailing space must not count as a "change" (which would fire an
      // unnecessary `PATCH`) and, for `url`, must not reach the server as the
      // `400`-inducing control/whitespace it rejects.
      const location = (values.location ?? '').trim()
      const hasLocation = location.length > 0
      const url = (values.url ?? '').trim()
      const hasUrl = url.length > 0
      setMutationError(null)
      savingRef.current = true
      setSaving(true)

      // Always clear the in-flight flag, on success or failure.
      const finish = () => {
        savingRef.current = false
        setSaving(false)
      }

      if (editing === null) {
        // A newly created event is anchored to the browser's current zone so a
        // recurrence repeats in local wall-clock time (F18).
        const tzid = browserTimeZone()
        const optimisticId = tempId()
        startTransition(async () => {
          addOptimistic({
            kind: 'create',
            event: {
              id: optimisticId,
              uid: optimisticId,
              collection_id: collectionId,
              summary: values.summary,
              description: hasDescription ? description : null,
              location: hasLocation ? location : null,
              url: hasUrl ? url : null,
              dtstart,
              dtend,
              dtstart_tzid: tzid ?? null,
              dtend_tzid: tzid ?? null,
              rrule: null,
              etag: '',
              created_at: Date.now(),
              updated_at: Date.now(),
            },
          })
          try {
            const input: CreateEventInput = { summary: values.summary, dtstart, dtend }
            if (hasDescription) {
              input.description = description
            }
            // On creation there is no previous value: only send what was filled.
            if (hasLocation) {
              input.location = location
            }
            if (hasUrl) {
              input.url = url
            }
            if (tzid !== undefined) {
              input.tzid = tzid
            }
            const created = await createEvent(collectionId, input)
            setEvents((previous) => [...previous, created])
            setModalOpen(false)
            message.success('Evento creado')
          } catch (error) {
            // Single error surface: the inline `Alert` (no duplicate toast).
            setMutationError(toErrorMessage(error))
          } finally {
            finish()
          }
        })
        return
      }

      const target = editing
      // Editing must NOT re-anchor the event: `dtstart` is a fixed instant, so
      // re-anchoring (say) a `Europe/Madrid` event to the browser zone would
      // silently change the local time of its recurrence. Preserve the existing
      // anchor; when the event has none, omit `tzid` rather than invent one.
      const anchorTzid = target.dtstart_tzid
      startTransition(async () => {
        addOptimistic({
          kind: 'update',
          uid: target.uid,
          changes: {
            summary: values.summary,
            description: hasDescription ? description : null,
            location: hasLocation ? location : null,
            url: hasUrl ? url : null,
            dtstart,
            dtend,
            dtstart_tzid: anchorTzid,
            dtend_tzid: target.dtend_tzid,
          },
        })
        try {
          const input: UpdateEventInput = { summary: values.summary, dtstart, dtend }
          // D2 (PLAN-028): `description`/`location`/`url` are sent **only** when
          // they differ from the value the dialog opened with. An old row written
          // by DAV may carry a `LOCATION`/`URL` the column does not know yet, so
          // the dialog shows it empty; sending it unconditionally would wipe it
          // silently. Clearing a field *is* a difference, so `""` is sent and the
          // server normalises it to `null` (which drops the `.ics` line) — that is
          // how O3 lets the user leave an event without description/location/URL.
          // The comparison is between *trimmed* values on both sides, so a legacy
          // value carrying a stray space that the user never touched counts as
          // "unchanged" and is preserved verbatim in the `.ics`.
          if (description !== (target.description ?? '').trim()) {
            input.description = description
          }
          if (location !== (target.location ?? '').trim()) {
            input.location = location
          }
          if (url !== (target.url ?? '').trim()) {
            input.url = url
          }
          if (anchorTzid !== null) {
            input.tzid = anchorTzid
          }
          const updated = await updateEvent(collectionId, target.uid, input)
          setEvents((previous) =>
            previous.map((event) => (event.uid === target.uid ? updated : event)),
          )
          setModalOpen(false)
          message.success('Evento actualizado')
        } catch (error) {
          setMutationError(toErrorMessage(error))
        } finally {
          finish()
        }
      })
    },
    [addOptimistic, editing, effectiveCollectionId, message, setEvents],
  )

  const handleDelete = useCallback(
    (event: CalendarEvent) => {
      const collectionId = effectiveCollectionId
      if (collectionId === null) {
        return
      }
      setMutationError(null)
      startTransition(async () => {
        addOptimistic({ kind: 'delete', uid: event.uid })
        try {
          await deleteEvent(collectionId, event.uid)
          setEvents((previous) => previous.filter((item) => item.uid !== event.uid))
          message.success('Evento eliminado')
        } catch (error) {
          // Single error surface: the inline `Alert` (no duplicate toast).
          setMutationError(toErrorMessage(error))
        }
      })
    },
    [addOptimistic, effectiveCollectionId, message, setEvents],
  )

  const loading = collectionsLoading || (effectiveCollectionId !== null && eventsLoading)
  const loadError = collectionsError ?? (effectiveCollectionId !== null ? eventsError : null)

  const formInitialValues: EventFormValues = useMemo(() => {
    if (editing !== null) {
      const start = editing.dtstart === null ? dayjs() : dayjs(editing.dtstart)
      const end = editing.dtend === null ? start.add(1, 'hour') : dayjs(editing.dtend)
      return {
        summary: editing.summary ?? '',
        description: editing.description ?? '',
        location: editing.location ?? '',
        url: editing.url ?? '',
        range: [start, end],
      }
    }
    return { summary: '', description: '', location: '', url: '', range: defaultRange() }
  }, [editing])

  // Baseline the URL validator compares against (D2): the value the dialog was
  // opened with, so an unchanged inherited `mailto:`/`ftp:` URL is accepted and
  // never has to be destroyed to be able to save.
  const urlBaseline = useMemo(() => (editing?.url ?? '').trim(), [editing])

  // La instancia `form` es única por vista y la comparte el `<Modal>` (su `onOk`
  // llama a `form.submit()`), así que sobrevive al remontaje por `key` del
  // formulario. Su *store* conserva los valores del evento anterior y Ant Design,
  // al reinicializar, hace `merge(initialValues, store)`: el *store* gana, de modo
  // que `initialValues` por sí solo no reemplaza nada. Hay que resetear
  // explícitamente cuando cambia el evento para que el formulario muestre los del
  // evento nuevo, no los del anterior (mientras el baseline D2 ya es el nuevo,
  // lo que enviaría o limpiaría la ubicación/URL equivocada).
  useEffect(() => {
    if (modalOpen) {
      form.resetFields()
    }
  }, [form, formInitialValues, modalOpen])

  return (
    <>
      <h1 className="sr-only">Calendario</h1>

      <Flex vertical gap={PAGE_GAP}>
        {loadError !== null ? (
          <Alert
            type="error"
            showIcon
            title="No se pudieron cargar los datos"
            description={loadError}
            action={
              <Button
                size="small"
                onClick={() => {
                  setCollectionsReloadToken((token) => token + 1)
                  reloadEvents()
                }}
              >
                Reintentar
              </Button>
            }
          />
        ) : null}

        {mutationError !== null ? (
          <Alert
            type="error"
            showIcon
            closable
            title={mutationError}
            onClose={() => setMutationError(null)}
          />
        ) : null}

        <Card
          loading={loading}
          title={formatMonthLabel(month)}
          extra={
            <Space wrap>
              <Select
                aria-label="Colección"
                style={fluidControlStyle}
                placeholder="Selecciona un calendario"
                value={effectiveCollectionId ?? undefined}
                onChange={(value: string) => setSelectedCollectionId(value)}
                loading={collectionsLoading}
                disabled={collections.length === 0}
                options={collections.map((collection) => ({
                  value: collection.id,
                  label: collection.name,
                }))}
              />
              <Button
                aria-label="Mes anterior"
                onClick={() => setMonth((current) => addMonths(current, -1))}
              >
                ‹
              </Button>
              <Button onClick={() => setMonth(() => new Date())}>Hoy</Button>
              <Button
                aria-label="Mes siguiente"
                onClick={() => setMonth((current) => addMonths(current, 1))}
              >
                ›
              </Button>
              <Button type="primary" onClick={openCreate} disabled={effectiveCollectionId === null}>
                Nuevo evento
              </Button>
            </Space>
          }
        >
          {collections.length === 0 ? (
            <Empty description="No hay calendarios disponibles" />
          ) : (
            <div style={{ overflowX: 'auto' }}>
              <div
                style={{
                  display: 'grid',
                  gridTemplateColumns: 'repeat(7, minmax(0, 1fr))',
                  gap: 4,
                  minWidth: 480,
                }}
              >
                {WEEKDAYS.map((weekday) => (
                  <div key={weekday} style={{ padding: 8, textAlign: 'center', fontWeight: 600 }}>
                    {weekday}
                  </div>
                ))}
                {days.map((day) => {
                  const key = dateKey(day)
                  const dayEvents = eventsByDay.get(key) ?? []
                  const inMonth = isSameMonth(day, month)
                  return (
                    <div
                      key={key}
                      style={{
                        minHeight: 96,
                        padding: 4,
                        border: '1px solid rgba(128, 128, 128, 0.25)',
                        borderRadius: 4,
                        background: inMonth ? undefined : 'rgba(128, 128, 128, 0.08)',
                        opacity: inMonth ? 1 : 0.6,
                      }}
                    >
                      <div style={{ fontSize: 12, fontWeight: 600, marginBottom: 4 }}>
                        {formatDayLabel(day)}
                      </div>
                      <ul
                        style={{
                          listStyle: 'none',
                          margin: 0,
                          padding: 0,
                          display: 'flex',
                          flexDirection: 'column',
                          gap: 2,
                        }}
                      >
                        {dayEvents.map((event) => (
                          <li
                            key={event.uid}
                            style={{ display: 'flex', alignItems: 'center', gap: 4, minWidth: 0 }}
                          >
                            <Button
                              type="link"
                              size="small"
                              style={{
                                padding: 0,
                                height: 'auto',
                                textAlign: 'left',
                                whiteSpace: 'normal',
                                minWidth: 0,
                              }}
                              onClick={() => openEdit(event)}
                            >
                              {formatTime(event.dtstart)} · {event.summary ?? '(sin título)'}
                            </Button>
                            {event.location !== null ? (
                              <Typography.Text
                                type="secondary"
                                style={{
                                  fontSize: 12,
                                  flex: '0 0 auto',
                                  maxWidth: 96,
                                  overflow: 'hidden',
                                  textOverflow: 'ellipsis',
                                  whiteSpace: 'nowrap',
                                }}
                              >
                                {event.location}
                              </Typography.Text>
                            ) : null}
                            {isSafeHttpUrl(event.url) ? (
                              <Typography.Link
                                href={event.url}
                                target="_blank"
                                rel="noopener noreferrer"
                                style={{
                                  fontSize: 12,
                                  flex: '0 0 auto',
                                  maxWidth: 96,
                                  overflow: 'hidden',
                                  textOverflow: 'ellipsis',
                                  whiteSpace: 'nowrap',
                                }}
                              >
                                {event.url}
                              </Typography.Link>
                            ) : null}
                            {event.dtstart_tzid !== null ? (
                              <Typography.Text
                                type="secondary"
                                style={{
                                  fontSize: 12,
                                  flex: '0 0 auto',
                                  maxWidth: 96,
                                  overflow: 'hidden',
                                  textOverflow: 'ellipsis',
                                  whiteSpace: 'nowrap',
                                }}
                              >
                                {event.dtstart_tzid}
                              </Typography.Text>
                            ) : null}
                            <Popconfirm
                              title="Eliminar evento"
                              description="¿Seguro que quieres eliminar este evento?"
                              okText="Eliminar"
                              cancelText="Cancelar"
                              okButtonProps={{ danger: true }}
                              onConfirm={() => handleDelete(event)}
                            >
                              <Button
                                danger
                                type="text"
                                size="small"
                                aria-label={`Eliminar ${event.summary ?? 'evento'}`}
                              >
                                ✕
                              </Button>
                            </Popconfirm>
                          </li>
                        ))}
                      </ul>
                    </div>
                  )
                })}
              </div>
            </div>
          )}
        </Card>
      </Flex>

      <Modal
        title={editing === null ? 'Nuevo evento' : 'Editar evento'}
        open={modalOpen}
        onOk={() => form.submit()}
        okText={editing === null ? 'Crear' : 'Guardar'}
        cancelText="Cancelar"
        confirmLoading={saving}
        maskClosable={!saving}
        keyboard={!saving}
        onCancel={() => {
          if (!saving) {
            setModalOpen(false)
          }
        }}
        destroyOnHidden
      >
        <Form
          form={form}
          layout="vertical"
          onFinish={handleSubmit}
          initialValues={formInitialValues}
          // `key` fuerza un montaje limpio al cambiar de evento (o de edición a
          // alta), para que `initialValues` se apliquen de nuevo y el formulario
          // no muestre los valores del evento anterior mientras el baseline D2 ya
          // es el nuevo.
          key={editing?.uid ?? 'new'}
        >
          <Form.Item
            name="summary"
            label="Título"
            rules={[{ required: true, message: 'Introduce un título' }]}
          >
            <Input placeholder="Título del evento" />
          </Form.Item>
          <Form.Item name="description" label="Descripción">
            <Input.TextArea rows={3} placeholder="Descripción (opcional)" />
          </Form.Item>
          <Form.Item name="location" label="Ubicación">
            <Input placeholder="Ubicación (opcional)" />
          </Form.Item>
          <Form.Item
            name="url"
            label="URL"
            rules={[
              {
                // The server allowlists the scheme (F24) but lets an inherited,
                // unchanged value through verbatim; `validateUrlValue` mirrors
                // both halves so the user gets immediate feedback instead of a
                // 400 — while a legacy `mailto:`/`ftp:` URL stays editable.
                validator: (_rule: unknown, value: unknown) => {
                  const error = validateUrlValue(value, urlBaseline)
                  return error === null ? Promise.resolve() : Promise.reject(new Error(error))
                },
              },
            ]}
          >
            <Input type="url" placeholder="https://ejemplo.com (opcional)" />
          </Form.Item>
          <Form.Item
            name="range"
            label="Fecha y hora"
            rules={[{ required: true, message: 'Selecciona un rango' }]}
          >
            <RangePicker
              showTime
              format="DD/MM/YYYY HH:mm:ss"
              style={{ width: '100%' }}
            />
          </Form.Item>
        </Form>
      </Modal>
    </>
  )
}
