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
  Checkbox,
  DatePicker,
  Empty,
  Flex,
  Form,
  Input,
  InputNumber,
  Modal,
  Popconfirm,
  Radio,
  Select,
  Skeleton,
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
import { useEventDetail } from '../hooks/useEventDetail'
import { useEvents } from '../hooks/useEvents'
import { toErrorMessage } from '../services/api'
import { listCalendarCollections } from '../services/collections'
import { createEvent, deleteEvent, updateEvent } from '../services/events'
import type {
  CalendarEvent,
  Collection,
  CreateEventInput,
  EventDetail,
  UpdateEventInput,
} from '../types/api'
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
import {
  FREQ_OPTIONS,
  WEEKDAY_OPTIONS,
  composeRrule,
  describeRrule,
  parseRrule,
  type RecurEndKind,
  type RecurFreq,
  type RecurrenceRule,
  type Weekday,
} from '../utils/recurrence'
import { describeRecurDates, sameInstantSet, sortUniqueInstants } from '../utils/recurDates'
import { availableTimeZones, browserTimeZone } from '../utils/timezone'
import { tempId } from '../utils/tempId'
import { PAGE_GAP, fluidControlStyle } from './pageLayout'

const { RangePicker } = DatePicker

/** Column headers of the month grid (Monday-first, matching `buildMonthGrid`). */
const WEEKDAYS = ['Lun', 'Mar', 'Mié', 'Jue', 'Vie', 'Sáb', 'Dom'] as const

const timeFormatter = new Intl.DateTimeFormat('es-ES', { hour: '2-digit', minute: '2-digit' })

function formatTime(ms: number | null): string {
  return ms === null ? 'Todo el día' : timeFormatter.format(new Date(ms))
}

// ---------------------------------------------------------------------------
// F25 — STATUS / CLASS / TRANSP white lists (mirror of the server, PLAN-029)
// ---------------------------------------------------------------------------

const STATUS_OPTIONS = [
  { value: 'TENTATIVE', label: 'Tentativo' },
  { value: 'CONFIRMED', label: 'Confirmado' },
  { value: 'CANCELLED', label: 'Cancelado' },
]

const CLASS_OPTIONS = [
  { value: 'PUBLIC', label: 'Público' },
  { value: 'PRIVATE', label: 'Privado' },
  { value: 'CONFIDENTIAL', label: 'Confidencial' },
]

const TRANSP_OPTIONS = [
  { value: 'OPAQUE', label: 'Ocupado (opaco)' },
  { value: 'TRANSPARENT', label: 'Libre (transparente)' },
]

/** Unit a composed `DURATION` is built from: `PT15M`, `PT2H`, `P1D`. */
type DurationUnit = 'minutes' | 'hours' | 'days'

const DURATION_UNIT_OPTIONS = [
  { value: 'minutes', label: 'minutos' },
  { value: 'hours', label: 'horas' },
  { value: 'days', label: 'días' },
]

/**
 * Decompose a `DURATION` into a single number + unit, when possible.
 *
 * Only the three shapes the form can compose are recognised (`P1D`, `PT2H`,
 * `PT15M`). Anything else (e.g. `P1DT2H`, `PT1H30M`) is **not** decomposable:
 * the dialog shows it verbatim and refuses to rewrite it unless the user
 * changes it — the rule that keeps D2 from silently turning a composed value
 * into an invented one.
 */
function decomposeDuration(raw: string): { value: number; unit: DurationUnit } | null {
  let match = /^P(\d+)D$/.exec(raw)
  if (match !== null) {
    return { value: Number(match[1]), unit: 'days' }
  }
  match = /^PT(\d+)H$/.exec(raw)
  if (match !== null) {
    return { value: Number(match[1]), unit: 'hours' }
  }
  match = /^PT(\d+)M$/.exec(raw)
  if (match !== null) {
    return { value: Number(match[1]), unit: 'minutes' }
  }
  return null
}

/** Compose an RFC 5545 `DURATION` from a positive integer + unit; `null` otherwise. */
function composeDuration(value: number | null | undefined, unit: string | undefined): string | null {
  if (value === null || value === undefined || unit === undefined) {
    return null
  }
  const size = Math.trunc(value)
  if (!Number.isFinite(size) || size <= 0) {
    return null
  }
  switch (unit) {
    case 'minutes':
      return `PT${size}M`
    case 'hours':
      return `PT${size}H`
    case 'days':
      return `P${size}D`
    default:
      return null
  }
}

/**
 * Milliseconds a `DURATION` stands for, or `null` when it cannot be read.
 *
 * Used **only to paint** an end on the card when the event carries no `DTEND`
 * (F14). The derived instant is never sent: `DURATION` and `DTEND` are
 * exclusive and the server only ever learns the user's explicit choice.
 */
function durationToMs(raw: string): number | null {
  const match = /^P(?:(\d+)W)?(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?$/.exec(raw)
  if (match === null) {
    return null
  }
  const [, weeks, days, hours, minutes, seconds] = match
  if (
    weeks === undefined &&
    days === undefined &&
    hours === undefined &&
    minutes === undefined &&
    seconds === undefined
  ) {
    return null
  }
  return (
    ((Number(weeks ?? 0) * 7 + Number(days ?? 0)) * 24 * 60 + Number(hours ?? 0) * 60 + Number(minutes ?? 0)) *
      60_000 +
    Number(seconds ?? 0) * 1000
  )
}

/** Structural equality used for D2's "only send what changed" on lists. */
function sameStringArray(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((value, index) => value === b[index])
}

/**
 * Build the edit form's initial values from the loaded detail (never from the
 * light list row, which lacks the six F25 properties).
 *
 * The duration radio opens on «Dura…» when the event carries a `DURATION`. A
 * decomposable value pre-fills the number+unit; a non-decomposable one (e.g.
 * `P1DT2H`) leaves them empty on purpose — the raw text is shown as-is and is
 * not rewritten unless the user changes it.
 */
function formValuesFromDetail(detail: EventDetail): EventFormValues {
  const start = detail.dtstart === null ? dayjs() : dayjs(detail.dtstart)
  let end = detail.dtend === null ? start.add(1, 'hour') : dayjs(detail.dtend)
  let durationMode: 'end' | 'duration' = 'end'
  let durationValue: number | undefined
  let durationUnit: DurationUnit | undefined
  if (detail.duration !== null) {
    durationMode = 'duration'
    const decomposed = decomposeDuration(detail.duration)
    if (decomposed !== null) {
      durationValue = decomposed.value
      durationUnit = decomposed.unit
      const ms = durationToMs(detail.duration)
      if (ms !== null) {
        end = start.add(ms, 'millisecond')
      }
    }
  }
  // F26 — recurrencia. Solo se precarga el constructor con una regla que el
  // subconjunto soportado entiende (`parseRrule`); una regla opaca deja el
  // constructor vacío: se muestra verbatim y solo se reescribe por acción
  // explícita del usuario (disciplina D2).
  let recurFreq: RecurFreq | undefined
  let recurInterval: number | undefined
  let recurEndKind: RecurEndKind | undefined
  let recurCount: number | undefined
  let recurUntil: Dayjs | undefined
  let recurByDay: Weekday[] | undefined
  if (detail.rrule !== null) {
    const rule = parseRrule(detail.rrule)
    if (rule !== null) {
      recurFreq = rule.freq
      recurInterval = rule.interval
      recurEndKind = rule.endKind
      if (rule.count !== undefined) {
        recurCount = rule.count
      }
      if (rule.until !== undefined) {
        recurUntil = untilToDayjs(rule.until)
      }
      if (rule.byDay !== undefined) {
        recurByDay = rule.byDay
      }
    }
  }
  return {
    summary: detail.summary ?? '',
    description: detail.description ?? '',
    location: detail.location ?? '',
    url: detail.url ?? '',
    range: [start, end],
    status: detail.status ?? undefined,
    class: detail.class ?? undefined,
    transp: detail.transp ?? undefined,
    categories: detail.categories,
    attach: detail.attach,
    durationMode,
    durationValue,
    durationUnit,
    tzid: detail.dtstart_tzid ?? undefined,
    recurFreq,
    recurInterval,
    recurEndKind,
    recurCount,
    recurUntil,
    recurByDay,
    // F27 — una propiedad opaca no se precarga en el constructor: se muestra
    // aparte, verbatim, y solo se reescribe por acción explícita (D2). Igual que
    // la recurrencia, el formulario arranca vacío si el baseline es opaco.
    exdate: detail.exdate_opaque === null ? detail.exdate.map((ms) => dayjs(ms)) : [],
    rdate: detail.rdate_opaque === null ? detail.rdate.map((ms) => dayjs(ms)) : [],
  }
}

/**
 * Read a `YYYYMMDD` `UNTIL` into a `Dayjs`. Built from an ISO `YYYY-MM-DD`
 * string so it does not depend on the `customParseFormat` plugin being loaded.
 */
function untilToDayjs(until: string): Dayjs {
  return dayjs(`${until.slice(0, 4)}-${until.slice(4, 6)}-${until.slice(6, 8)}`)
}

/**
 * Coerce an antd numeric control value to a positive integer, or `undefined`.
 *
 * `InputNumber` emits `null` when cleared and (without `precision`) accepts
 * decimals; the server's `RRULE` alphabet rejects `.`, so anything that is not
 * a finite integer `>= 1` is treated as "absent" (mirrors `composeDuration`).
 */
function normalizePositiveInt(value: number | null | undefined): number | undefined {
  if (value === null || value === undefined) {
    return undefined
  }
  const size = Math.trunc(value)
  return Number.isFinite(size) && size >= 1 ? size : undefined
}

/**
 * Build the structured recurrence from the form values (F26); `null` when no
 * frequency is chosen. `interval` defaults to 1 and `BYDAY` is only meaningful
 * for a weekly rule (mirrors `composeRrule`).
 *
 * Total: nunca lanza ni compone un fin inválido. Un `count`/`until` ausente o
 * borrado (antd emite `null`) hace caer el fin a `never`, de modo que jamás se
 * envía `COUNT=null`/`UNTIL=undefined` al servidor.
 */
function ruleFromValues(values: EventFormValues): RecurrenceRule | null {
  const freq = values.recurFreq
  if (freq === undefined) {
    return null
  }
  const interval = normalizePositiveInt(values.recurInterval) ?? 1
  const requestedEnd: RecurEndKind = values.recurEndKind ?? 'never'
  const rule: RecurrenceRule = { freq, interval, endKind: 'never' }
  if (requestedEnd === 'count') {
    const count = normalizePositiveInt(values.recurCount)
    if (count !== undefined) {
      rule.endKind = 'count'
      rule.count = count
    }
  } else if (requestedEnd === 'until') {
    const until = values.recurUntil
    if (until !== null && until !== undefined) {
      rule.endKind = 'until'
      rule.until = until.format('YYYYMMDD')
    }
  }
  if (freq === 'WEEKLY' && values.recurByDay !== undefined && values.recurByDay.length > 0) {
    rule.byDay = values.recurByDay
  }
  return rule
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
  status?: string
  class?: string
  transp?: string
  categories?: string[]
  attach?: string[]
  /** Which end wins: an explicit `dtend` (`end`) or a `duration` (`duration`). */
  durationMode: 'end' | 'duration'
  /** Magnitude for the composed `DURATION`; empty when not decomposable. */
  durationValue?: number
  /** Unit for the composed `DURATION`. */
  durationUnit?: DurationUnit
  /** F26 — IANA time zone anchoring the event (`TZID`); empty means none. */
  tzid?: string
  /** F26 — recurrence frequency; absent means "no recurrence". */
  recurFreq?: RecurFreq
  /** F26 — every how many periods (antd can emit `null`). */
  recurInterval?: number | null
  /** F26 — how the recurrence ends (default `never`). */
  recurEndKind?: RecurEndKind
  /** F26 — number of occurrences (antd emits `null` when cleared). */
  recurCount?: number | null
  /** F26 — last day (antd emits `null` when cleared). */
  recurUntil?: Dayjs | null
  /** F26 — weekdays (only for a weekly rule). */
  recurByDay?: Weekday[]
  /** F26 — explicit "remove recurrence" for an opaque baseline rule. */
  clearRecurrence?: boolean
  /** F27 — instantes excluidos de la recurrencia (`EXDATE`), ya parseados. */
  exdate?: Dayjs[]
  /** F27 — instantes extra de la recurrencia (`RDATE`). */
  rdate?: Dayjs[]
  /** F27 — «quitar» explícito sobre una propiedad opaca (baseline no modelable). */
  clearExdate?: boolean
  clearRdate?: boolean
}

/** Fields the optimistic layer may rewrite on an existing event. */
type EventChanges = Pick<
  CalendarEvent,
  | 'summary'
  | 'description'
  | 'location'
  | 'url'
  | 'dtstart'
  | 'dtend'
  | 'dtstart_tzid'
  | 'dtend_tzid'
  | 'rrule'
> &
  Partial<
    Pick<
      EventDetail,
      'status' | 'class' | 'transp' | 'categories' | 'attach' | 'duration' | 'exdate' | 'rdate'
    >
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

/** Sentinel `baseline` that no real value can equal, forcing full validation. */
const NO_ATTACH_BASELINE = '\u0000'

/**
 * Validate one `ATTACH` value with the **same** scheme/whitespace frontier as
 * {@link validateUrlValue}, but per item of a list.
 *
 * An inherited value (one of the URIs the dialog opened with) is accepted
 * verbatim, mirroring the server: an `ATTACH:mailto:…` an imported `.ics`
 * already carries is preserved. A value the user changed or added is a new
 * boundary crossing and must be an absolute `http://`/`https://` URI.
 */
function validateAttachValue(value: unknown, baselines: string[]): string | null {
  if (typeof value !== 'string') {
    return null
  }
  const trimmed = value.trim()
  if (trimmed === '' || baselines.includes(trimmed)) {
    return null
  }
  return validateUrlValue(trimmed, NO_ATTACH_BASELINE)
}

type OptimisticAction =
  | { kind: 'create'; event: CalendarEvent & Partial<EventDetail> }
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

  // El diálogo de edición necesita el detalle **completo** (la fila ligera de la
  // lista no trae `categories`/`attach`/`status`/…), así que tiene su propia
  // petición, independiente de la lista. En alta (`editing === null`) no pide nada.
  // El `etag` entra en el objetivo para que guardar invalide el detalle cacheado:
  // tras el `PATCH` la lista consolida la fila con un `etag` nuevo, y reabrir el
  // mismo evento vuelve a pedir el detalle en lugar de mostrar el obsoleto.
  const editTarget = useMemo(() => {
    if (editing === null || effectiveCollectionId === null) {
      return null
    }
    return { collectionId: effectiveCollectionId, uid: editing.uid, etag: editing.etag }
  }, [editing, effectiveCollectionId])

  const {
    detail: editDetail,
    loading: editDetailLoading,
    error: editDetailError,
    reload: reloadEditDetail,
  } = useEventDetail(editTarget)

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
      // F25 fields. The scalars use `''` as "unset" (the server clears on a
      // blank value); the lists compare by value (D2).
      const status = values.status ?? ''
      const eventClass = values.class ?? ''
      const transp = values.transp ?? ''
      const categories = values.categories ?? []
      const attach = (values.attach ?? [])
        .map((item) => (item ?? '').trim())
        .filter((item) => item.length > 0)
      // `DURATION`/`DTEND` are exclusive (RFC 5545 / plan's D2): the radio decides
      // which one is authoritative and the other is *never* sent.
      const durationMode = values.durationMode ?? 'end'
      const composedDuration =
        durationMode === 'duration'
          ? composeDuration(values.durationValue, values.durationUnit)
          : null
      setMutationError(null)
      savingRef.current = true
      setSaving(true)

      // Always clear the in-flight flag, on success or failure.
      const finish = () => {
        savingRef.current = false
        setSaving(false)
      }

      if (editing === null) {
        // F26 — the zone is a first-class, clearable field now (defaulting to
        // the browser's zone); an empty value means "no zone" and the key is
        // omitted (the API rejects an empty `tzid`).
        const tzid = (values.tzid ?? '').trim()
        const rule = ruleFromValues(values)
        const rrule = rule !== null ? composeRrule(rule) : null
        // F27 — listas en forma **canónica** (orden ascendente, sin duplicados),
        // igual que las serializa el servidor.
        const exdateInstants = sortUniqueInstants(
          (values.exdate ?? []).filter((d): d is Dayjs => d != null).map((d) => d.valueOf()),
        )
        const rdateInstants = sortUniqueInstants(
          (values.rdate ?? []).filter((d): d is Dayjs => d != null).map((d) => d.valueOf()),
        )
        const optimisticId = tempId()
        // With a duration the optimistic event paints the end derived locally;
        // this instant is display-only and is never sent.
        const durationMs = composedDuration === null ? null : durationToMs(composedDuration)
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
              dtend: durationMode === 'end' ? dtend : durationMs === null ? null : dtstart + durationMs,
              dtstart_tzid: tzid === '' ? null : tzid,
              dtend_tzid: tzid === '' ? null : tzid,
              rrule,
              etag: '',
              created_at: Date.now(),
              updated_at: Date.now(),
              status: status === '' ? null : status,
              class: eventClass === '' ? null : eventClass,
              transp: transp === '' ? null : transp,
              categories,
              attach,
              duration: durationMode === 'duration' ? composedDuration : null,
              exdate: rule !== null ? exdateInstants : [],
              rdate: rule !== null ? rdateInstants : [],
            },
          })
          try {
            const input: CreateEventInput = { summary: values.summary, dtstart }
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
            if (tzid !== '') {
              input.tzid = tzid
            }
            if (rrule !== null) {
              input.rrule = rrule
            }
            if (durationMode === 'end') {
              input.dtend = dtend
            } else if (composedDuration !== null) {
              input.duration = composedDuration
            }
            if (status !== '') {
              input.status = status
            }
            if (eventClass !== '') {
              input.class = eventClass
            }
            if (transp !== '') {
              input.transp = transp
            }
            if (categories.length > 0) {
              input.categories = categories
            }
            if (attach.length > 0) {
              input.attach = attach
            }
            // F27 — EXDATE/RDATE forman parte de la recurrencia: solo se envían
            // si hay una regla. Si el usuario limpia la frecuencia en alta, el
            // bloque se oculta (pero antd conserva sus valores) y estas fechas
            // NO deben viajar, o el servidor recibiría fechas sin `RRULE`.
            if (rule !== null && exdateInstants.length > 0) {
              input.exdate = exdateInstants
            }
            if (rule !== null && rdateInstants.length > 0) {
              input.rdate = rdateInstants
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
      const original = editDetail
      // The form is only mounted once the detail has loaded, so this is a
      // defensive guard rather than a reachable path.
      if (original === null) {
        finish()
        return
      }
      // F26 — the zone can be changed but not removed (a `PATCH` cannot clear
      // it): an empty field keeps the current anchor, a non-empty one is sent.
      const tzid = (values.tzid ?? '').trim()
      const optimisticTzid = tzid !== '' ? tzid : target.dtstart_tzid
      // F26 — recurrence follows D2: only send it when it actually changed. An
      // opaque baseline (`parseRrule` ⇒ `null`) is preserved verbatim unless the
      // user composes a new rule or ticks «Quitar recurrencia».
      const composed = ruleFromValues(values)
      const composedStr = composed !== null ? composeRrule(composed) : null
      const baselineRrule = original.rrule
      // M1 — se compara contra la forma **canónica** del baseline, no contra el
      // texto crudo: una regla parseable pero no canónica (`BYDAY` desordenado,
      // `UNTIL` corto, `INTERVAL=1` explícito) no debe reescribirse si el
      // formulario reconstruye exactamente la misma regla. Así el `.ics` conserva
      // el texto original verbatim.
      const baselineParsed = baselineRrule !== null ? parseRrule(baselineRrule) : null
      const baselineCanonical = baselineParsed !== null ? composeRrule(baselineParsed) : null
      let rruleInput: string | undefined
      let optimisticRrule: string | null = baselineRrule
      if (values.clearRecurrence === true) {
        rruleInput = ''
        optimisticRrule = null
      } else if (composedStr !== null && composedStr !== baselineCanonical) {
        rruleInput = composedStr
        optimisticRrule = composedStr
      } else if (
        composedStr === null &&
        baselineRrule !== null &&
        parseRrule(baselineRrule) !== null
      ) {
        // The user emptied a rule the form had decomposed: clear it.
        rruleInput = ''
        optimisticRrule = null
      }
      // F27 — `EXDATE`/`RDATE` siguen D2: solo viajan si cambian respecto del
      // baseline. Un baseline **opaco** se preserva verbatim salvo que el usuario
      // marque el checkbox; uno modelable compara por conjunto de instantes.
      const formExdate = sortUniqueInstants(
        (values.exdate ?? []).filter((d): d is Dayjs => d != null).map((d) => d.valueOf()),
      )
      const formRdate = sortUniqueInstants(
        (values.rdate ?? []).filter((d): d is Dayjs => d != null).map((d) => d.valueOf()),
      )
      const baselineExdate = original.exdate
      const baselineRdate = original.rdate
      let exdateInput: number[] | undefined
      let optimisticExdate: number[] = baselineExdate
      if (values.clearExdate === true) {
        exdateInput = []
        optimisticExdate = []
      } else if (!sameInstantSet(formExdate, baselineExdate)) {
        exdateInput = formExdate
        optimisticExdate = formExdate
      }
      let rdateInput: number[] | undefined
      let optimisticRdate: number[] = baselineRdate
      if (values.clearRdate === true) {
        rdateInput = []
        optimisticRdate = []
      } else if (!sameInstantSet(formRdate, baselineRdate)) {
        rdateInput = formRdate
        optimisticRdate = formRdate
      }
      // `DURATION` is only sent when the user actually changed it. A value that
      // does not decompose into one number+unit (e.g. `P1DT2H`) is shown verbatim
      // and left untouched — sending a recomposed/rounded value would rewrite it
      // silently. Switching to «Termina a las…» sends `dtend` instead, and the
      // server drops the `DURATION` (they are exclusive).
      let durationInput: string | undefined
      if (durationMode === 'duration') {
        if (composedDuration !== null) {
          if (composedDuration !== (original.duration ?? '')) {
            durationInput = composedDuration
          }
        } else if (original.duration !== null && decomposeDuration(original.duration) !== null) {
          // The user emptied a duration the form had decomposed: clear it.
          durationInput = ''
        }
      }
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
            dtend: durationMode === 'end' ? dtend : null,
            dtstart_tzid: optimisticTzid,
            dtend_tzid: tzid !== '' ? tzid : target.dtend_tzid,
            rrule: optimisticRrule,
            status: status === '' ? null : status,
            class: eventClass === '' ? null : eventClass,
            transp: transp === '' ? null : transp,
            categories,
            attach,
            duration: durationMode === 'duration' ? composedDuration : null,
            exdate: optimisticExdate,
            rdate: optimisticRdate,
          },
        })
        try {
          const input: UpdateEventInput = { summary: values.summary, dtstart }
          if (durationMode === 'end') {
            input.dtend = dtend
          } else if (durationInput !== undefined) {
            input.duration = durationInput
          }
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
          if (description !== (original.description ?? '').trim()) {
            input.description = description
          }
          if (location !== (original.location ?? '').trim()) {
            input.location = location
          }
          if (url !== (original.url ?? '').trim()) {
            input.url = url
          }
          // F25: only the six that changed travel (D2); an emptier value than the
          // baseline counts as a change, so clearing means sending `''`/`[]`.
          if (status !== (original.status ?? '')) {
            input.status = status
          }
          if (eventClass !== (original.class ?? '')) {
            input.class = eventClass
          }
          if (transp !== (original.transp ?? '')) {
            input.transp = transp
          }
          if (!sameStringArray(categories, original.categories)) {
            input.categories = categories
          }
          if (!sameStringArray(attach, original.attach)) {
            input.attach = attach
          }
          if (tzid !== '') {
            input.tzid = tzid
          }
          if (rruleInput !== undefined) {
            input.rrule = rruleInput
          }
          if (exdateInput !== undefined) {
            input.exdate = exdateInput
          }
          if (rdateInput !== undefined) {
            input.rdate = rdateInput
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
    [addOptimistic, editDetail, editing, effectiveCollectionId, message, setEvents],
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
    // En edición los valores salen del **detalle** cargado (no de la fila ligera
    // de la lista, que no trae las seis propiedades F25). Mientras el detalle no
    // ha llegado el `<Form>` ni se monta, así que estos valores siempre son los
    // definitivos cuando el formulario aparece.
    if (editing !== null && editDetail !== null) {
      return formValuesFromDetail(editDetail)
    }
    return {
      summary: '',
      description: '',
      location: '',
      url: '',
      range: defaultRange(),
      durationMode: 'end',
      // F26 — a new event defaults to the browser zone; the recurrence stays
      // off until a frequency is chosen.
      tzid: browserTimeZone(),
      recurInterval: 1,
      recurEndKind: 'never',
      exdate: [],
      rdate: [],
      clearExdate: false,
      clearRdate: false,
    }
  }, [editing, editDetail])

  // Baseline the URL validator compares against (D2): the value the dialog was
  // opened with, so an unchanged inherited `mailto:`/`ftp:` URL is accepted and
  // never has to be destroyed to be able to save. It comes from the detail, not
  // the light row — the dialog fills from the detail.
  const urlBaseline = useMemo(() => (editDetail?.url ?? '').trim(), [editDetail])

  // Same idea for the `ATTACH` list: an inherited non-http URI is preserved, so
  // the per-item validator must accept the values the dialog opened with.
  const attachBaselines = useMemo(() => editDetail?.attach ?? [], [editDetail])

  // F26 — the zone selector offers every known IANA zone and, defensively, the
  // event's current zone even when the runtime does not list it.
  const timezoneOptions = useMemo(() => {
    const zones = availableTimeZones()
    const current = editDetail?.dtstart_tzid ?? null
    const all = current !== null && !zones.includes(current) ? [...zones, current] : zones
    return all.map((zone) => ({ value: zone, label: zone }))
  }, [editDetail])

  // F26 — a recurrence the form cannot influence (opaque) is shown verbatim and
  // preserved unless the user composes a new one or ticks «Quitar recurrencia».
  const opaqueRrule = useMemo(() => {
    const raw = editDetail?.rrule ?? null
    return raw !== null && parseRrule(raw) === null ? raw : null
  }, [editDetail])

  // F27 — `EXDATE`/`RDATE` opacos (no modelables) se muestran verbatim y se
  // preservan salvo que el usuario los vacíe con el checkbox (D2).
  const exdateOpaque = useMemo(() => editDetail?.exdate_opaque ?? null, [editDetail])
  const rdateOpaque = useMemo(() => editDetail?.rdate_opaque ?? null, [editDetail])
  // ¿El evento (en edición) trae ya fechas de recurrencia? Si es así, el bloque
  // F27 se muestra aunque no haya `recurFreq` (p. ej. una serie definida solo por
  // `RDATE`, o un baseline importado por un cliente DAV).
  const hasRecurDates = useMemo(() => {
    const d = editDetail
    if (d === null) return false
    return (
      d.exdate.length > 0 ||
      d.rdate.length > 0 ||
      d.exdate_opaque !== null ||
      d.rdate_opaque !== null
    )
  }, [editDetail])

  // A `DURATION` the form cannot decompose is shown verbatim; while it is
  // untouched it must not be sent (D2), so the dialog advertises it explicitly.
  const nonDecomposableDuration = useMemo(() => {
    const raw = editDetail?.duration ?? null
    return raw !== null && decomposeDuration(raw) === null ? raw : null
  }, [editDetail])

  // El formulario de edición no se monta hasta tener el detalle completo; así
  // `initialValues` son los definitivos y `form.resetFields()` sigue funcionando.
  const editDetailPending = editing !== null && (editDetailLoading || editDetail === null)
  const editDetailFailed = editing !== null ? editDetailError : null

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
        // Con el detalle de edición aún cargando el formulario no existe, así que
        // Guardar no haría nada útil: se bloquea hasta tener los valores.
        okButtonProps={{ disabled: editDetailPending }}
        maskClosable={!saving}
        keyboard={!saving}
        onCancel={() => {
          if (!saving) {
            setModalOpen(false)
          }
        }}
        // El formulario es largo: el cuerpo hace scroll dentro del modal.
        width={720}
        styles={{ body: { maxHeight: '70vh', overflowY: 'auto', paddingRight: 12 } }}
        destroyOnHidden
      >
        {editDetailFailed !== null ? (
          <Alert
            type="error"
            showIcon
            title="No se pudo cargar el evento para editar"
            description={editDetailFailed}
            action={
              <Button size="small" onClick={reloadEditDetail}>
                Reintentar
              </Button>
            }
          />
        ) : editDetailPending ? (
          <Skeleton active paragraph={{ rows: 8 }} />
        ) : (
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

            {/* F25 — estado, visibilidad y transparencia. */}
            <Space wrap align="start" size="middle">
              <Form.Item name="status" label="Estado" style={{ minWidth: 160 }}>
                <Select allowClear placeholder="Sin definir" options={STATUS_OPTIONS} />
              </Form.Item>
              <Form.Item name="class" label="Visibilidad" style={{ minWidth: 160 }}>
                <Select allowClear placeholder="Sin definir" options={CLASS_OPTIONS} />
              </Form.Item>
              <Form.Item name="transp" label="Transparencia" style={{ minWidth: 200 }}>
                <Select allowClear placeholder="Sin definir" options={TRANSP_OPTIONS} />
              </Form.Item>
            </Space>

            <Form.Item name="categories" label="Categorías">
              <Select
                mode="tags"
                allowClear
                tokenSeparators={[',']}
                placeholder="trabajo, personal…"
                aria-label="Categorías"
                suffixIcon={null}
              />
            </Form.Item>

            {/* `ATTACH`: lista de URLs con la misma frontera de esquema que `URL`. */}
            <Form.List name="attach">
              {(fields, { add, remove }) => (
                <Flex vertical gap="small" style={{ marginBottom: 16 }}>
                  <Typography.Text strong>Adjuntos</Typography.Text>
                  {fields.map((field) => (
                    <Flex key={field.key} gap="small" align="end">
                      <Form.Item
                        name={field.name}
                        style={{ flex: 1, marginBottom: 0 }}
                        rules={[
                          {
                            validator: (_rule: unknown, value: unknown) => {
                              const error = validateAttachValue(value, attachBaselines)
                              return error === null
                                ? Promise.resolve()
                                : Promise.reject(new Error(error))
                            },
                          },
                        ]}
                      >
                        <Input
                          type="url"
                          placeholder="https://ejemplo.com/adjunto"
                          aria-label={`Adjunto ${field.name + 1}`}
                        />
                      </Form.Item>
                      <Button
                        danger
                        type="text"
                        onClick={() => remove(field.name)}
                        aria-label={`Quitar adjunto ${field.name + 1}`}
                      >
                        Quitar
                      </Button>
                    </Flex>
                  ))}
                  <Button onClick={() => add('')} aria-label="Añadir adjunto">
                    Añadir adjunto
                  </Button>
                </Flex>
              )}
            </Form.List>

            {/* Radio que traduce la exclusión `DURATION`/`DTEND` (RFC 5545). */}
            <Form.Item name="durationMode" label="Duración">
              <Radio.Group>
                <Radio value="end">Termina a las…</Radio>
                <Radio value="duration">Dura…</Radio>
              </Radio.Group>
            </Form.Item>
            <Form.Item
              noStyle
              shouldUpdate={(previous, current) => previous.durationMode !== current.durationMode}
            >
              {({ getFieldValue }) =>
                getFieldValue('durationMode') === 'duration' ? (
                  <Flex vertical gap="small" style={{ marginBottom: 16 }}>
                    <Flex gap="small" align="end" wrap>
                      <Form.Item name="durationValue" label="Cantidad" style={{ marginBottom: 0 }}>
                        <InputNumber min={1} style={{ width: 120 }} aria-label="Cantidad" />
                      </Form.Item>
                      <Form.Item name="durationUnit" label="Unidad" style={{ marginBottom: 0 }}>
                        <Select
                          style={{ width: 140 }}
                          options={DURATION_UNIT_OPTIONS}
                          aria-label="Unidad"
                        />
                      </Form.Item>
                    </Flex>
                    {nonDecomposableDuration !== null ? (
                      <Typography.Text type="secondary">
                        Duración actual: {nonDecomposableDuration}
                      </Typography.Text>
                    ) : null}
                  </Flex>
                ) : null
              }
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

            {/* F26 — zona horaria (`TZID`). Limpiable solo en alta: el `PATCH`
                no puede quitar la zona (un `tzid` vacío es un 400). */}
            <Form.Item
              name="tzid"
              label="Zona horaria"
              extra={
                editing !== null ? (
                  <Typography.Text type="secondary">
                    La zona no se puede quitar desde aquí
                  </Typography.Text>
                ) : undefined
              }
            >
              <Select
                showSearch
                allowClear={editing === null}
                placeholder="UTC / sin zona"
                aria-label="Zona horaria"
                optionFilterProp="label"
                options={timezoneOptions}
              />
            </Form.Item>

            {/* F26 — recurrencia (`RRULE`) con constructor estructurado. */}
            {opaqueRrule !== null ? (
              <Flex vertical gap="small" style={{ marginBottom: 16 }}>
                <Typography.Text type="secondary">
                  Recurrencia actual: {opaqueRrule}
                </Typography.Text>
                <Form.Item name="clearRecurrence" valuePropName="checked" noStyle>
                  <Checkbox>Quitar recurrencia</Checkbox>
                </Form.Item>
              </Flex>
            ) : null}

            <Form.Item name="recurFreq" label="Repetición">
              <Select
                showSearch
                allowClear
                placeholder="Sin repetición"
                aria-label="Repetición"
                optionFilterProp="label"
                options={[...FREQ_OPTIONS]}
              />
            </Form.Item>

            <Form.Item noStyle shouldUpdate>
              {(formInstance) => {
                const freq = formInstance.getFieldValue('recurFreq') as RecurFreq | undefined
                // F27 — el bloque también aparece si el evento trae `exdate`/
                // `rdate` (p. ej. una serie definida solo por `RDATE`) aunque no
                // haya `recurFreq`.
                if (freq === undefined && !hasRecurDates) {
                  return null
                }
                const endKind = formInstance.getFieldValue('recurEndKind') as
                  | RecurEndKind
                  | undefined
                const values = formInstance.getFieldsValue() as EventFormValues
                const preview = ruleFromValues(values)
                const previewText = preview !== null ? describeRrule(composeRrule(preview)) : null
                // F27 — resumen en vivo de las fechas ya seleccionadas.
                const exdateSummary = describeRecurDates(
                  (values.exdate ?? []).filter((d): d is Dayjs => d != null).map((d) => d.valueOf()),
                )
                const rdateSummary = describeRecurDates(
                  (values.rdate ?? []).filter((d): d is Dayjs => d != null).map((d) => d.valueOf()),
                )
                return (
                  <Flex vertical gap="small" style={{ marginBottom: 16 }}>
                    {/* Controles dependientes de la frecuencia. */}
                    {freq !== undefined ? (
                      <>
                        <Flex gap="small" align="end" wrap>
                          <Form.Item name="recurInterval" label="Cada" style={{ marginBottom: 0 }}>
                            <InputNumber
                              min={1}
                              precision={0}
                              style={{ width: 120 }}
                              aria-label="Cada"
                            />
                          </Form.Item>
                          {freq === 'WEEKLY' ? (
                            <Form.Item
                              name="recurByDay"
                              label="Días de la semana"
                              style={{ minWidth: 260, marginBottom: 0 }}
                            >
                              <Select
                                mode="multiple"
                                allowClear
                                placeholder="Lunes, miércoles…"
                                aria-label="Días de la semana"
                                optionFilterProp="label"
                                options={[...WEEKDAY_OPTIONS]}
                              />
                            </Form.Item>
                          ) : null}
                        </Flex>
                        <Form.Item name="recurEndKind" label="Fin" style={{ marginBottom: 0 }}>
                          <Radio.Group aria-label="Fin de la repetición">
                            <Radio value="never">Nunca</Radio>
                            <Radio value="count">Después de N veces</Radio>
                            <Radio value="until">Hasta</Radio>
                          </Radio.Group>
                        </Form.Item>
                        {endKind === 'count' ? (
                          <Form.Item
                            name="recurCount"
                            label="Número de veces"
                            style={{ marginBottom: 0 }}
                            rules={[{ required: true, message: 'Indica cuántas veces se repite' }]}
                          >
                            <InputNumber
                              min={1}
                              precision={0}
                              style={{ width: 140 }}
                              aria-label="Número de veces"
                            />
                          </Form.Item>
                        ) : null}
                        {endKind === 'until' ? (
                          <Form.Item
                            name="recurUntil"
                            label="Hasta el día"
                            style={{ marginBottom: 0 }}
                            rules={[{ required: true, message: 'Indica la fecha de fin' }]}
                          >
                            <DatePicker aria-label="Hasta el día" />
                          </Form.Item>
                        ) : null}
                        {previewText !== null ? (
                          <Typography.Text type="secondary">{previewText}</Typography.Text>
                        ) : null}
                      </>
                    ) : null}

                    {/* F27 — EXDATE. Si el baseline es opaco se muestra verbatim
                        y solo se puede vaciar con el checkbox (D2). */}
                    {exdateOpaque !== null ? (
                      <Flex vertical gap="small" style={{ marginBottom: 16 }}>
                        <Typography.Text type="secondary">
                          Fechas excluidas actuales: {exdateOpaque}
                        </Typography.Text>
                        <Form.Item name="clearExdate" valuePropName="checked" noStyle>
                          <Checkbox>Quitar fechas excluidas</Checkbox>
                        </Form.Item>
                      </Flex>
                    ) : (
                      // F27 — antd conserva los valores del `Form.List` aunque el
                      // bloque se desmonte (p. ej. al limpiar «Repetición» en alta).
                      // No se limpian aquí: el filtro vive en `handleSubmit`, que
                      // solo envía EXDATE si hay una regla (`rule !== null`).
                      <Form.List name="exdate">
                        {(fields, { add, remove }) => (
                          <Flex vertical gap="small" style={{ marginBottom: 16 }}>
                            <Typography.Text strong>Fechas excluidas</Typography.Text>
                            {fields.map((field) => (
                              <Flex key={field.key} gap="small" align="end">
                                <Form.Item name={field.name} style={{ flex: 1, marginBottom: 0 }}>
                                  <DatePicker
                                    showTime
                                    format="DD/MM/YYYY HH:mm:ss"
                                    style={{ width: '100%' }}
                                    aria-label={`Fecha excluida ${field.name + 1}`}
                                  />
                                </Form.Item>
                                <Button
                                  danger
                                  type="text"
                                  onClick={() => remove(field.name)}
                                  aria-label={`Quitar fecha excluida ${field.name + 1}`}
                                >
                                  Quitar
                                </Button>
                              </Flex>
                            ))}
                            <Button onClick={() => add()} aria-label="Añadir fecha excluida">
                              Añadir fecha excluida
                            </Button>
                            {exdateSummary !== '' ? (
                              <Typography.Text type="secondary">{exdateSummary}</Typography.Text>
                            ) : null}
                          </Flex>
                        )}
                      </Form.List>
                    )}

                    {/* F27 — RDATE. Análogo a EXDATE. */}
                    {rdateOpaque !== null ? (
                      <Flex vertical gap="small" style={{ marginBottom: 16 }}>
                        <Typography.Text type="secondary">
                          Fechas extra actuales: {rdateOpaque}
                        </Typography.Text>
                        <Form.Item name="clearRdate" valuePropName="checked" noStyle>
                          <Checkbox>Quitar fechas extra</Checkbox>
                        </Form.Item>
                      </Flex>
                    ) : (
                      // F27 — mismo motivo que en `exdate`: el filtro para no
                      // enviar RDATE sin `RRULE` está en `handleSubmit`.
                      <Form.List name="rdate">
                        {(fields, { add, remove }) => (
                          <Flex vertical gap="small" style={{ marginBottom: 16 }}>
                            <Typography.Text strong>Fechas extra</Typography.Text>
                            {fields.map((field) => (
                              <Flex key={field.key} gap="small" align="end">
                                <Form.Item name={field.name} style={{ flex: 1, marginBottom: 0 }}>
                                  <DatePicker
                                    showTime
                                    format="DD/MM/YYYY HH:mm:ss"
                                    style={{ width: '100%' }}
                                    aria-label={`Fecha extra ${field.name + 1}`}
                                  />
                                </Form.Item>
                                <Button
                                  danger
                                  type="text"
                                  onClick={() => remove(field.name)}
                                  aria-label={`Quitar fecha extra ${field.name + 1}`}
                                >
                                  Quitar
                                </Button>
                              </Flex>
                            ))}
                            <Button onClick={() => add()} aria-label="Añadir fecha extra">
                              Añadir fecha extra
                            </Button>
                            {rdateSummary !== '' ? (
                              <Typography.Text type="secondary">{rdateSummary}</Typography.Text>
                            ) : null}
                          </Flex>
                        )}
                      </Form.List>
                    )}
                  </Flex>
                )
              }}
            </Form.Item>
          </Form>
        )}
      </Modal>
    </>
  )
}
