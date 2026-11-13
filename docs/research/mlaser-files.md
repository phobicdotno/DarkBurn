# Mlaser machine package: file formats (2026-11-13)

What the files in a Gweike M3 Ultra machine package (the folder the vendor ships with the machine, and the `File` folder of an Mlaser install) contain, and how `darkburn-mcc` reads them. Written from the owner's own package. No value, serial number or path from that package is copied here; the examples are made up.

Legend: **observed** = seen in the files; **inferred** = my reading of what was seen; **open** = not established.

Code: `crates/darkburn-mcc/src/paramxml.rs`, `recipe.rs`, `profile.rs`, `mlaser_files.rs`, `library.rs`. Nothing in these modules opens a port, a socket or a device; they read and write files only, and the machine settings are only read.

## Package layout (observed)

```
<package>/
  Cutting parameters/
    1200W/<material>/<name>.xml        fibre recipes cut with N2 or O2
    1200W_air/<material>/<name>.xml    fibre recipes cut with air
    CO2/<material>/<name>.xml          CO2 recipes
  Cleaning and quenching/<name>.xml    two fibre recipes for nozzle cleaning and quenching
  1390backup.xml                       full machine backup (all settings in one document)
  pwmCompensation.txt                  fibre scan compensation table
  Co2_pwmCompensation.txt              CO2 scan compensation table
```

An Mlaser install has, in its `File` folder: `BkHardPara.xml`, `BkManuPara.xml`, `BkLayerPara.xml`, `SecondBkManuPara.xml`, `softPara.ini`, `ipAdd.ini`, `ManuContour.dat`, `PithCompensate.pcf`, `AutosaveParam*.ini`, `Temp/tempIsBreak.ini`, `ProcessesStatistic.txt` and `.chf` job files.

The scanner (`library::scan`) finds all of these at any depth, so either folder (or a parent of both) can be pointed at.

## Parameter XML (observed)

All `.xml` files above share one small dialect:

- A root element `<ParameterRoot>`, section elements under it (`<PLayerParam11>`, `<PManuParam>`, ...), and self-closing entry elements in each section (`<GP .../>`, `<MC .../>`).
- One element per line, CRLF line ends, a trailing CRLF, UTF-8 without a byte-order mark, no XML declaration.
- Numbers are written like C's `%.17g` (`0.59999999999999998`), integers plain. `paramxml::format_g17` writes the same form.
- Entry names are not unique across sections: `DO`, `MGP` and `GP` each appear in more than one section. An entry is therefore addressed by section, name and its position among same-named entries.

`ParamDoc` keeps every element and attribute in order with the text as stored, so a file read and written back is byte-identical, and a file with a few changed attributes differs only in those attributes. The parser is hand-written, accepts a declaration, comments and open/close entries, and refuses deeper nesting and text content with an error. Comments are dropped.

## Recipes and layer tables

A recipe file holds one layer section with one `GP` entry: `PLayerParam11` for the fibre head, `PCO2LayerParam11` for the CO2 head (observed: every recipe in the package uses slot 11). A layer table (`BkLayerPara.xml`, and the layer part of the backup) holds `PLayerParam1` to `PLayerParam11` and `PCO2LayerParam1` to `PCO2LayerParam11`; section `n` is layer index `n - 1`.

The fibre `GP` entry has 164 attributes; the craft model covers about 90 of them. The CO2 entry has 20 in recipe files and 21 in layer tables, which add `CutFreq` (a recipe without it reads as 5000 Hz).

Fibre mapping (`recipe::fibre_from_entry`):

| Craft field | Attributes |
|---|---|
| cut | `CutHeight`, `CutFocusPos`, `CutSpeed`, `CutPower`, `CutFreq`, `CutPeakCurrent`, `CutGasType`, `CutAirPressure`, `LaserOnDelay`, `LaserOffBeforeDelay`, `LaserOffAfterDelay`, `UpHeight` |
| follower | `NoFollow` (inverted), `AdvFixHeightCutPos`, `ShortDistNoUp`, `PreDrill`, `PreDrillIsNotUp`, `AfterPreDrillMustDrillBeforeCut`, `WithFilm` |
| pierce stage `i` (0 to 4) | `DrillHeight{i}`, `DrillFocusPos{i}`, `DrillPower{i}`, `DrillFreq{i}`, `DrillPeakCurrent{i}`, `DrillGasType{i}`, `DrillGasPressure{i}`, `DrillDelay{i}` (pierce time), `EnableGradualDrill{i}` + `GradualTime{i}`, `EnableFocusGradual{i}` + `FocusGradualEndPos{i}` + `FocusGradualTime{i}`, `BeforeLaserOffDelay{i}`, `AfterLaserOffDelay{i}` |
| process | `ManuType` (0 direct cut, 1 fixed height, 2 to 4 one to three pierce stages) |
| other | `SlowStart*`, `EnableSmoothPierce` + `SmoothPierce*`, `LayerFileName` (name), `Note`, `NoManu`, `ShortDistGasKeepOn`, `NoCloseGasInManu`, `PWMCurveNodes`, `FreqCurveNodes`, `PowerAdjustWithSpeed`, `FreqAdjustWithSpeed` |

CO2 mapping: `CutSpeed`, `CutDuty`, `CutFreq`, `CutGasType`, `CutAirPressure`, `LaserOnDelay`, `AdvFixHeightCutPos`, the two curves and their switches, `SlowStart*`, `LayerFileName`, `Note`, `ShortDistGasKeepOn`, `NoCloseGasInManu`.

Gas codes: 0 air, 1 O2, 2 N2, 3 high-pressure air, 4 high-pressure O2, 5 high-pressure N2.

Writing a craft back changes only the attributes whose field changed. An attribute missing from the file stays missing unless its value changed; a new recipe is built from a template entry (`fibre_template`, `co2_template`) with the vendor's default values.

Decisions:

- All five stored pierce stages are read, whatever `ManuType` says; `ProcessType::stages` says how many run.
- The nozzle is taken from the recipe name (below), not from an attribute.

### Recipe names (observed)

Names follow `<material> <thickness>mm <nozzle><S|D> F<focus> <gas> [<pressure>]`, for example `Steel 3.0mm 1.2D F+13 O2` or `Brass 2mm 1.5S F-3 air 1Mpa`, with uneven spacing. `S` and `D` are single and double nozzles. `RecipeName::parse` reads each part when present.

What the names say and what the file holds do not always agree (observed across the whole package):

- **Focus**: every fibre recipe stores `CutFocusPos = 0` while its name gives `F-1` to `F+13`. **Inferred**: the name records the focus the head was set to by hand; the stored value is not used on this machine.
- **Air recipes** (`1200W_air`) store gas code 5 (high-pressure N2). **Inferred**: air is fed on the nitrogen line. The scanner reports this as information, not a warning.
- **Pressure**: fibre recipes store `CutAirPressure` between about 0.35 and 1.0, names say `1Mpa`, and the cleaning and quenching recipes (named 1.0 and 0.6 MPa) both store 0.85. CO2 recipes store 5. **Open**: the unit. The craft field is named `gas_pressure_bar`, and the checks allow up to 10; the fibre values read naturally as MPa. Nothing converts between the two.

The scanner (`library::check_recipe`) reports these differences per recipe as `Check`s, apart from the craft checks.

### Craft checks on the owner's package

All 55 recipes (40 fibre cutting, 13 CO2, 2 cleaning and quenching) and the layer tables of the backup and `BkLayerPara.xml` pass the existing craft checks (`craft::FibreCraft::validate`, `Co2Craft::validate`) with no error and no warning. This was checked with a read-only script that applies the same rules; the test `library::tests::real_package_from_env` runs the real code on a package when `DARKBURN_MLASER_DIR` names one.

## Machine settings (`Bk*Para.xml`, backup)

The backup holds every settings section in one document. The Mlaser install splits the same sections over:

- `BkHardPara.xml`: axes, homing, laser, gas, inputs, outputs, follower and so on;
- `BkManuPara.xml`: manual and job settings (`PManuParam`, `PSoftParam`, `PAFParam`, `PDOParam`, `PZFParam`, `PECParam`, `PGraphParam`, `PNestParam`, `PImportGraphParam`);
- `BkLayerPara.xml`: the layer tables;
- `SecondBkManuPara.xml`: a second copy of the manual settings (**inferred**: a backup the program keeps).

Some entries are split between files too: `PSoftParam` `SOP`, `SP` and `GP` have part of their attributes in `BkHardPara.xml` and the rest in `BkManuPara.xml`.

`MachineProfile` merges documents attribute by attribute, keyed by (section, entry, position among same-named entries). Decision: **the first source wins**; a later source with another value is kept as a `Conflict` naming both sources. The scanner ranks files named `Bk*` first, then other names containing `bk`, then the rest, so the install's own files win over a backup in the same folder.

Typed views (read only): `PAxisParam`, `PHomeParam`, `PZFParam` (follower), `PLaserParam`, `PManuParam` (jog), `PGasParam`, `PDOParam`, `PDIParam`, `PFCParam` (soft limits and speeds), `PGraphParam`. Every other section stays in `MachineProfile::doc` untouched, and `untyped_sections` lists them.

Notes on the typed parts:

- **Axes**: `PAxisParam/A<n>` with `PHomeParam/A<n>` or `HPA<n>`. `PMachineAxisConfig_<n>` (`MAC`, `MAC_n`) holds travel, pulses per turn and limits per motor. **Open**: how `PMachineAxisConfig_<n>` maps onto `A<n>`.
- **Inputs** (`PDIParam`): `EStop`, `WaterWarning`, `LaserWarning`, `CO2WaterWarning`, `CO2LaserWarning`, each with a `*Type`. Port `0` means not wired. `AlarmDIStr` and `FunctionDIStr` are lists of `name#port#-1#0` items (`NamedPort`); names may be Chinese. **Open**: the meaning of the `*Type` level codes, including `CO2WaterWarningType`; `co2_water_input()` returns the port (1 to 28) and leaves the level to the caller.
- **Outputs** (`PDOParam`): alarm, ring, wait and job-running outputs, `CustomDOStr`.
- **Gas** (`PGasParam/MGP`, `PManuParam/GC`): one valve output per gas code, the proportional-valve analogue ports (`RatioAir`, `RatioO2`, `RatioH2`, sic) and `DAMaxPressure`.
- **Laser**: fibre and CO2 control types, analogue ports, gate, enable and red-light outputs.
- `machine_params()` fills DarkBurn's `MachineParams` from `PManuParam/MC`, `MP` and `PFCParam/FCP` (`MaxSpeed`, `MaxAcc`), with a note for every value missing.

`MachineProfile::diff` lists every attribute that differs between two profiles and marks the volatile ones (counters, last file paths, device names, the action after a job) so a comparison can hide them.

## Scan compensation (`pwmCompensation.txt`, `Co2_pwmCompensation.txt`)

One `a,b` pair per line, ascending in `a` (observed). The backup holds the same pairs as `FiberScanFlyCompensateStr` and `CO2ScanFlyCompensateStr` in the form `100#0.34,200#0.56,...`.

**Inferred, not confirmed**: `a` is the speed in mm/s and `b` an offset in mm that corrects the lag of the laser behind the motion during fly (scan) cutting. `b` could also be a time. `CompensationTable::offset_at` interpolates linearly and holds the end values. The scanner reports where a text table and the profile's string differ.

## Small files

- `softPara.ini`, section `[SC2000]`: `NormalExit` and the last positions `XAxis`, `YAxis`, `ZAxis`, `WAxis` in card units (**inferred**: 1000 units per mm). Read as `SoftPara`.
- `ipAdd.ini`, section `[IP]`: endpoints given as `<Prefix>IP`, `<Prefix>Subnet`, `<Prefix>Gateway`, `<Prefix>Port`, plus timing keys. Read as `NetSettings` for display only.
- `scFlie` lists (sic, observed): a first line `scFlie`, values one per line, a last line `eof`. `ManuContour.dat` is a count followed by that many values. `PithCompensate.pcf`, `AutosaveParam*.ini` and `Temp/tempIsBreak.ini` use the same frame; their meaning is **open**. Read as `ScFile`, which keeps any text after `eof`.
- `ProcessesStatistic.txt`: one job per line, comma-separated (name, size, two lengths, a count, a time, a date; the middle fields' meaning is **open**). The text is code page 936. Decision: decoding Chinese needs the crate feature `cp936` (as for `.chf` files); without it those characters become U+FFFD.
- `.chf` jobs are listed by the scanner, not read; `chf.rs` reads them.

`IniFile` keeps lines, comments, order, line ends and encoding, so an edited file differs only in the edited lines. Every small-file reader refuses input over 1 MiB and never panics.

## Scanner limits

`library::scan` reads files only. It does not follow symbolic links, stops 12 folders deep and after 50 000 directory entries, and lists files over 16 MiB as unreadable without reading them. A file it cannot read is listed in `Library::unreadable` with the reason. Recipes are sorted by path.

## Not done here

No UI yet: the quick switching between recipes on the M3 Ultra page builds on `Library::recipes`, `RecipeEntry::craft` and `Library::preset_craft`. Writing settings back to a machine is out of scope (owner decision Q046: Gweike is file export plus the simulator).
