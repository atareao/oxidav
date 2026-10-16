# Changelog
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
