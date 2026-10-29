# Changelog
## [0.7.0] - 2026-10-29

### Bug Fixes

- La tarjeta no pinta lo que la lista no lleva y el detalle se refresca

### Documentation

- Regenerar el changelog para 0.6.0
- Registra PLAN-029 (F25), contenido y estado del VEVENT
- Cierra PLAN-029 (F25) y documenta la feature

### Features

- Proyectar el detalle del VEVENT desde el payload
- Escribir CATEGORIES, ATTACH, STATUS, CLASS, TRANSP y DURATION
- El detalle del evento se proyecta al leer y la API hereda las seis
- El diálogo de edición, con categorías, adjuntos y estado

### Refactor

- Convertir dav/ics.rs en dav/ics/mod.rs

### Testing

- Cobertura de API para las seis y traza del fallo de proyeccion
## [0.6.0] - 2026-10-17

### Documentation

- Regenerar el changelog para 0.5.0
- El flujo de release dice la verdad
- Registra PLAN-028 (F24), la conservación del .ics al editar
- Cierra PLAN-028 (F24) y documenta la feature

### Features

- El PATCH conserva el .ics y LOCATION/URL son campos de primera clase
## [0.5.0] - 2026-10-16

### Bug Fixes

- El harness de tests acota su directorio y respeta TMPDIR

### Documentation

- Regenerar el changelog para 0.4.0
- Añadir PLAN-025 (partir dav/xml.rs y auth/session.rs)
- Cierre de PLAN-025 (desviaciones y un error mio cazado por la verificacion)
- Añadir PLAN-026 (el diálogo de edición, con pestañas, F23)
- Documentación de F23 y cierre de PLAN-026
- Cierra PLAN-027 con los resultados medidos

### Features

- El diálogo de edición va con pestañas, como el detalle (F23)

### Refactor

- Extraer la serialización a dav/xml/write.rs (PLAN-025, paso 1)
- Extraer el parseo a dav/xml/parse.rs (PLAN-025, paso 2)
- Extraer la configuración a auth/session/config.rs (PLAN-025, paso 3)
- Extraer el bloque OIDC a auth/session/oidc.rs (PLAN-025, paso 4a)
- Extraer los tokens propios a auth/session/tokens.rs (PLAN-025, paso 4b)
## [0.4.0] - 2026-10-16

### Bug Fixes

- Media type honesto en las fotos embebidas y cobertura de render_list_property (F22)
- Arreglos de la auditoría (N, import atómica, uid parseado, guardas y plegado) (F22)
- Borrar N, GENDER, BDAY y ANNIVERSARY desde el PATCH (F22)
- *(web)* Arreglos de la auditoría React (F22)

### Documentation

- Regenerar el changelog para 0.3.1
- Añadir PLAN-023 (vCard completo y vista por pestañas, F22)
- Documentación de F22 y cierre de PLAN-023
- Añadir PLAN-024 (partir los ficheros grandes de F22)
- Cierre de PLAN-024 y corrección del dato del baseline
- Corregir dos cifras obsoletas del cierre de PLAN-024

### Features

- Detalle completo de la vCard y escritura por merge en la API (F22)
- *(web)* Tipos, servicio y hook del detalle del contacto (F22)
- *(web)* Pestañas del detalle del contacto (F22)
- *(web)* Detalle real en el Drawer e importación en servidor (F22)
- *(web)* Formulario de contacto completo con PATCH por diferencia (F22)

### Refactor

- Mover src/dav/vcf.rs a src/dav/vcf/ (F22)
- Extraer la proyección a vcf/projection.rs (PLAN-024, paso 1)
- Extraer la composición a vcf/build.rs (PLAN-024, paso 2)
- Un solo parseo en project_card (PLAN-024, paso 3)
- Partir api/contacts.rs en mod/validate/edits (PLAN-024, paso 4)

### Testing

- Contrato de la API de detalle, merge y importación (F22)
## [0.3.1] - 2026-10-16

### Documentation

- Regenerar el changelog para 0.3.0
- Añadir PLAN-021 (limpieza de tokens de diseño, F20)
- Enmendar PLAN-021 tras la auditoría (F20)
- Documentar F20 y dar PLAN-021 por cerrado
- Corregir los enlaces obsoletos a los planes de origen en F17–F19
- Añadir PLAN-022 (proyecto de TypeScript para los tests, F21)
- Actualizar los comentarios que citaban la configuración anterior (F21)
- Documentar F21 y dar PLAN-022 por cerrado
- Registrar en PLAN-022 la enmienda por auditoría (F21)

### Refactor

- Fuente única de tokens y limpieza del CSS muerto (F20)
- Separar el proyecto de TypeScript de los tests (F21)

### Testing

- Invariantes de tokens y pila de fuentes única (F20)
- Invariantes de la separación de proyectos de TypeScript (F21)
- Exigir que los tsconfig parseen sin diagnósticos (F21)
## [0.3.0] - 2026-10-16

### Bug Fixes

- El color-scheme del documento sigue el tema de la app, no el del SO
- Recuperar el espaciado del aviso del modal de token creado

### Documentation

- Regenerate the changelog for 0.2.0
- Record the hard-won lessons in AGENTS.md
- Documentar F18 y levantar la limitación de zonas horarias de F07
- Plan del rediseño UX transversal del SPA (F19)
- Documentar F19 (rediseño UX transversal del SPA)
- Cerrar el plan de F19 (rediseño UX transversal)

### Features

- Soporte de zonas horarias (TZID + VTIMEZONE) extremo a extremo
- Sistema de diseno base del SPA (tokens y dependencia de iconos)
- Navegación en cabecera y shell responsive (sin Sider)
- H1 semántico oculto y retirada de los encabezados de página
- Estados homogéneos, responsividad y Alert migrado a title
- Título del documento por ruta y guardas de accesibilidad
## [0.2.0] - 2026-10-15

### Documentation

- Add generated CHANGELOG.md
- Close the session loose ends

### Features

- Replace bearer OIDC with the server-side session flow

### Miscellaneous Tasks

- Adopt gitflow with valet parity

### Testing

- Reset the OIDC caches so the discover tests stop flaking
## [0.1.1] - 2026-10-14

### Build

- Switch the Docker image to musl/Alpine 3.23

### Miscellaneous Tasks

- Pin the runners to ubuntu-26.04
## [0.1.0] - 2026-10-13

### Bug Fixes

- *(docker)* Seed web/dist stub before the dependency-cache build

### Documentation

- Add project README
- Record F16 delivery polish and close PLAN-017

### Features

- Implement OxiDav CalDAV/CardDAV/WebDAV server with embedded SPA
- *(web)* Honor the post-login redirect target

### Miscellaneous Tasks

- Add GitHub Actions workflow
- Fix web/dist artifact download path
- Bump GitHub Actions to latest majors
- Build and smoke-test the Docker image
- License the project under MIT
- Publish the image to GHCR

### Performance

- *(web)* Code-split routes and vendor chunks
