# Vendored Data Package schema

`datapackage-v2.schema.json` is an offline copy of the official Data Package v2
profile from <https://datapackage.org/profiles/2.0/datapackage.json>.

Semantic Engine embeds it in exported context packages so third-party JSON Schema
validation does not require network access. Update it deliberately, review the
upstream diff and keep its SHA-256 covered by export contract tests.
