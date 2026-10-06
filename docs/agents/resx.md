# Resources in Eludite: a guide for agents

Eludite's resource commands (`eludite.resx.*`) are the `.resx` editor's actions. A `.resx` file opens as its whole
resource set: the neutral file (`Resources.resx`) and the culture files beside it (`Resources.de.resx`,
`Resources.fr-FR.resx`), one grid with a row per key and a column per culture. The person sees every change in that grid.

## 1. Read first

- `eludite.resx.sets` lists the workspace's sets (or one project's with `project`, or the set a `path` belongs to):
  the neutral file, the cultures with their files and counts (strings, missing cells, warnings), the designer file
  and its access modifier.
- `eludite.resx.entries` reads a set's rows: each key with its value and comment in every culture (the neutral
  culture is the empty string), `missing` cells, the rule warnings and whether the key is invariant. Filter with
  `query` (key, value or comment), `missing`, `warnings` or `invariant`, pick `cultures`, and page with `skip` and
  `take` (500 by default).
- `eludite.resx.validate` runs the consistency rules alone: placeholders (`{0}`, `{name}`, `%s`), leading and
  trailing punctuation and white space must match the neutral value's, and a translation equal to the neutral value
  is flagged as untranslated. Warnings never refuse anything.

Name a set by any of its files' paths (absolute, or relative to the workspace folder).

## 2. Write

- `eludite.resx.set` writes cells: `{ set, key, culture, value?, comment?, invariant? }`, any number at once. A cell
  in a culture file that has no entry for the key is created on the fly; `value: null` removes a culture's entry
  (never the neutral one); `invariant: true` marks the key with `{Invariant}` in the neutral comment, so it counts
  as never translated. A culture file that does not exist yet is refused unless `create_culture` is true.
- `eludite.resx.add` adds a key with its neutral value; `eludite.resx.rename` renames a key in every file;
  `eludite.resx.remove` removes keys from every file.
- `eludite.resx.access_modifier` sets the designer class to `internal`, `public` or `none` (no designer).

The files keep every byte you did not change, and a new entry takes Visual Studio's shape at the end of the file.
The host regenerates `Resources.Designer.cs` after a key is added, removed or renamed. Code that uses a renamed key
is not touched: rename its uses with `eludite.editor.rename` on the designer property.

## 3. Translating

Read the missing cells with `entries` and `missing: true`, then write them with one `set` call holding every
cell. Keep the neutral value's placeholders, punctuation and white space, skip invariant keys, and never copy the
neutral value into a culture: the untranslated rule flags it.

## 4. What needs permission

Reads are always allowed. `set`, `add`, `rename` and `access_modifier` are edits: under the default policy they are
held as pending changes the person accepts or rejects in the grid (`pending` in the answer), or applied at once
under `edit_buffer: accept`. `remove` asks first (`resx.remove: prompt`, the default); `allow` makes it an ordinary
edit and `deny` refuses it, with the policy named in the error.
