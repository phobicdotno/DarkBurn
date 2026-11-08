# Galvo: things to confirm on a real board

`darkburn-galvo` was built from public descriptions of EzCad2 / BJJCZ boards and tested against an
in-memory emulator only. The owner has no galvo. Each line needs a check on real hardware (a
safe, red-light-only first) before the matching checklist item is ticked.

## Link and bring-up
- [ ] USB open on Windows with the WinUSB driver (VID 9588, PID 9899), interface 0, endpoints 0x02 / 0x88
- [ ] Reply length is always 8 bytes, including to `GetSerialNo`; `WriteCorLine` really sends no reply
- [ ] Init order works without `EnableLaser` and without `EnableZ` (both are left out by default)
- [ ] `GetVersion` reply: which word holds the version; serial number layout
- [ ] Whether the board needs `EnableLaser` before the galvo will move for a red-light frame
- [ ] READY / BUSY semantics: how many list packets the board buffers, and that `ExecuteList` after three packets is right
- [ ] `GetListStatus` paused bit (0x08) while a list is held with `StopList`; `RestartList` resumes at the same command
- [ ] `ReadPort` reply: input word is word 1
- [ ] `GetPositionXY` reply layout (assumed x, y in words 0 and 1)

## Lens correction
- [ ] `.cor` grid orientation: row = Y, column = X (assumed); sign of the offset (position plus offset assumed)
- [ ] A real vendor `.cor` file loads (both `LMC1COR_1.0` and the older int layout) and the board accepts the uploaded table
- [ ] Table node spacing 65535 / 64 and bilinear interpolation matches the board
- [ ] The lens scale value in the `.cor` header (index 43 in the float layout) is the field scale
- [ ] Lens-correction test pattern marked on card measures square after a correct `.cor`

## Field and calibration
- [ ] Field centre is 0x8000 and the full range is 0..65535 for the lens size entered
- [ ] Orientation: which of flip X, flip Y, swap XY and rotation a typical fibre head needs; sense of "rotate" (clockwise quarter turns assumed)
- [ ] Galvo 1 / 2 scale, skew, bulge, trapezoid, negate, is-X: DarkBurn's equations are its own (see `field.rs`); check they can correct a measured distortion
- [ ] Scale calibration: marked 100 mm square measures 100 mm after `scaled_for_measurement`
- [ ] Field offset sign

## Laser parameters
- [ ] Fibre: `QSwitchPeriod = 20000 / kHz` and `MarkCurrent` range 0..0xFFF give the expected frequency and power
- [ ] MOPA: `FiberYlpmPulseWidth` word units (nanoseconds assumed) and the allowed pairs of frequency and pulse width
- [ ] CO2 / UV: `MarkFreq = 10000 / kHz`, `MarkPowerRatio = 200 * power / kHz`, UV power invert, first pulse killer word
- [ ] Fibre MO delay at job start (800 units of 10 us assumed)
- [ ] Speed word: galvo units per millisecond really equals mm/s x units-per-mm / 1000 for this firmware

## Timings
- [ ] Units of `JumpDelay`, `LaserOnDelay`, `LaserOffDelay` (microseconds assumed), `PolygonDelay` and `DelayTime` (10 us units assumed)
- [ ] Negative laser on / off delays accepted (sign word 0x8000)
- [ ] End TC is applied as a `DelayTime` after the last mark of each path (assumed)
- [ ] Jump distance limit switches between min and max jump delay as intended

## Ports and safety
- [ ] Red light is output bit 8 and works with `WritePort`; jumps move the pointer with the laser dark
- [ ] Mark bit 0 high during a job is correct for the board, and never required for a frame
- [ ] Door switch input bit and polarity (default: not configured)
- [ ] Foot switch is input bit 15, active low
- [ ] Done-marking output bit, polarity and pulse length
- [ ] `StopExecute` + `FiberSetMo(false)` + `ResetList` + `LaserSignalOff` + `WritePort(0)` + `DisableLaser` really stops emission immediately on every laser type (do with the lens cap on and no material)
- [ ] `StopList` pauses at once and the laser goes dark when MO is closed

## Motion extras
- [ ] External axis: `SetAxisMotionParam` / `SetAxisOriginParam` parameter layout (not sent yet), `MoveAxisTo` position word (16-bit magnitude plus invert flag assumed), steps per mm, AXIS status bit
- [ ] Z axis: `EnableZ` + `SetZData` word meaning and sign
- [ ] Rotary: design Y maps to surface travel; windows of the chosen size stay in the field
- [ ] Repeat marking with external axis advances the right way

## Fill and wobble
- [ ] Flood fill reading (continuous sweeps with delay-free jumps) matches what operators expect
- [ ] Auto-rotate reading (passes spread over 180 degrees)
- [ ] Wobble: circle radius and step feel (points per revolution, step distance) on a real cut
- [ ] 3D sliced image: slice thresholds and focus step direction

## EZCad import
- [ ] Which keys real EZCad `markcfg` / ini files use for field size, offsets and galvo calibration (the importer matches by loose names; a binary `MARKCFG7` is refused)
- [ ] Whether a text export of EZCad's parameters exists and how it names the lens file
