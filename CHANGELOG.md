# Changelog
## [unreleased]

### Miscellaneous Tasks

- Adopt gitflow with valet parity
## [0.1.1] - 2026-10-14

### Miscellaneous Tasks

- Pin the runners to ubuntu-26.04

### Other

- Switch the Docker image to musl/Alpine 3.23
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
