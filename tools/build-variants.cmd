@echo off
REM ===========================================================================
REM  Build the Planck Duos receiver for ALL FOUR boards, from one source tree.
REM
REM  Usage:  tools\build-variants.cmd        (run from anywhere; it cds itself)
REM
REM  The four boards:
REM    52840-dongle    - the Nordic PCA10059 dongle. This is the board the
REM                      receiver was developed on, and it gets NO board file:
REM                      the root board.toml IS its layout, so a plain
REM                      `cargo build --release` is the 52840-dongle build.
REM                      Nordic USB DFU bootloader, app at 0x1000, flash it
REM                      with nRF Connect Programmer.
REM    nicenano-52840  - nice!nano, Adafruit UF2 bootloader (nosd), same 0x1000
REM                      origin, flash by dragging the .uf2.
REM    52833-dk        - PCA10100 / nRF52833 Dongle: no bootloader at all, so the
REM                      app starts at 0x0 and it is flashed over SWD. No .uf2 is
REM                      produced for it - a UF2 would be refused.
REM    nicenano-52833  - nice!nano 52833, Adafruit UF2 bootloader, app at 0x27000.
REM
REM  A board is a BUILD INPUT here, not a source edit: the variant names an
REM  OVERRIDE file through BOARD_OVERRIDE, and build.rs deep-merges it onto
REM  board.toml (it declares rerun-if-env-changed=BOARD_OVERRIDE, so switching
REM  really does rebuild the layout instead of reusing a stale target/
REM  directory). board.toml stays the single source of truth for the keyboard and
REM  the link - the matrix, the channel table, the USB identity and the Vial ID
REM  are inherited by every variant and cannot drift between them.
REM
REM  The build order groups the two nRF52840 boards and then the two nRF52833
REM  boards, so cargo rebuilds the dependency graph twice rather than four times.
REM
REM  Every product is verified afterwards by tools\verify_variant.py, which
REM  checks each image against the partition map it claims - because a `set`
REM  that silently did not take effect produces a correctly NAMED and wrongly
REM  LINKED image, and that combination flashes fine and then never runs.
REM ===========================================================================
setlocal
REM QMK_MSYS holds this machine's only arm-none-eabi-gcc. It is required, not
REM optional: rmk's BLE/crypto path (p256-cortex-m4-sys, pulled in by the
REM adafruit_bl feature) shells out to a C compiler while it builds, and if the
REM tool is not on PATH that build script dies with
REM "failed to find tool arm-none-eabi-gcc". GitHub Actions gets it from
REM apt-get gcc-arm-none-eabi instead; if it is already on your PATH, the
REM condition below leaves you alone.
if exist "C:\QMK_MSYS\mingw64\bin" set "PATH=C:\QMK_MSYS\mingw64\bin;C:\QMK_MSYS\opt\qmk\bin;C:\QMK_MSYS\usr\bin;%PATH%"

cd /d "%~dp0.." || exit /b 1

REM How a hex becomes a drag-and-drop uf2 is resolved in :make_uf2 at the bottom
REM of this file, so that this script contains no path that exists on one machine
REM only.

REM   variant           override file ("" = board.toml as-is)      chip args                                     uf2 family
call :build 52840-dongle   ""                                     ""                                              ""
if errorlevel 1 exit /b 1
call :build nicenano-52840 "boards\nice-nano-52840.toml"         ""                                              nrf52840
if errorlevel 1 exit /b 1
call :build 52833-dk       "boards\nrf52833-dongle.toml"         "--no-default-features --features nrf52833"     ""
if errorlevel 1 exit /b 1
call :build nicenano-52833 "boards\nice-nano-52833.toml"         "--no-default-features --features nrf52833"     nrf52833
if errorlevel 1 exit /b 1

echo.
echo === verifying the four products against the layouts they claim ===
python tools\verify_variant.py || exit /b 1

echo.
echo   flash with:
echo     gazell-dongle-52840-dongle.hex        nRF Connect Programmer (PCA10059 dongle)
echo     gazell-dongle-nicenano-52840.uf2      drag onto the nice!nano 52840 UF2 drive
echo     gazell-dongle-52833-dk.hex            SWD only - this board has no bootloader
echo     gazell-dongle-nicenano-52833.uf2      drag onto the nice!nano 52833 UF2 drive
echo.
echo   Do not erase all flash when flashing the two dongles: the USB DFU
echo   bootloader at 0xE0000 is the only way back without a debug probe.
endlocal
exit /b 0


REM ---------------------------------------------------------------------------
REM  :build [variant] [override-file, or empty] [chip-args] [uf2-family, or empty]
REM
REM  setlocal/endlocal around each board is load-bearing, not decoration:
REM  BOARD_OVERRIDE has to disappear again, or the next board inherits the
REM  previous board's layout and silently ships the wrong partition map.
REM  BOARD_TOML is also cleared, so a value left over in the calling shell cannot
REM  replace the root file that every variant is meant to start from.
REM ---------------------------------------------------------------------------
:build
setlocal
set "VARIANT=%~1"
set "OVERRIDE=%~2"
set "CHIPARGS=%~3"
set "FAMILY=%~4"
set "PROD=gazell-dongle-%VARIANT%"
set "BOARD_TOML="
if "%OVERRIDE%"=="" (set "BOARD_OVERRIDE=") else (set "BOARD_OVERRIDE=%OVERRIDE%")

echo.
echo === %VARIANT% : override=%OVERRIDE%  chip-args=%CHIPARGS% ===

cargo build --release %CHIPARGS% || exit /b 1
cargo objcopy --release %CHIPARGS% -- -O ihex %PROD%.hex || exit /b 1

REM The .hex is kept even for the UF2 boards: verify_variant.py compares the two
REM formats of the same image, and a conversion that moved the start address is
REM exactly the kind of thing that flashes and then does not run.
if not "%FAMILY%"=="" call :make_uf2 "%PROD%" "%FAMILY%" || exit /b 1

endlocal
exit /b 0


REM ---------------------------------------------------------------------------
REM  :make_uf2 [product-base-name] [nrf52840 or nrf52833]
REM
REM  Three ways to convert, tried in this order, because which one exists says
REM  nothing about the project and everything about the machine:
REM    one, the UF2CONV variable, if you set it explicitly
REM    two, a qmk_firmware or vial-qmk checkout under your home directory
REM    three, the cargo hex-to-uf2 subcommand, which is what CI uses
REM
REM  If none is available the hex is still built and verified, and the missing
REM  uf2 is reported as a NOTE rather than a failure: this repository does not
REM  vendor the conversion script, and inventing a path to somebody else's copy
REM  would only work on the machine it was written on.
REM
REM  uf2conv.py takes the family as a number and cargo hex-to-uf2 takes it as a
REM  name, so both forms live here rather than in the caller.
REM ---------------------------------------------------------------------------
:make_uf2
set "PROD=%~1"
set "FAM=%~2"
if "%FAM%"=="nrf52840" (set "FAMNUM=0xADA52840") else (set "FAMNUM=0x621E937A")

if defined UF2CONV goto :uf2_explicit
if exist "%USERPROFILE%\vial-qmk\util\uf2conv.py" set "UF2CONV=%USERPROFILE%\vial-qmk\util\uf2conv.py"
if exist "%USERPROFILE%\qmk_firmware\util\uf2conv.py" set "UF2CONV=%USERPROFILE%\qmk_firmware\util\uf2conv.py"

:uf2_explicit
if not defined UF2CONV goto :uf2_cargo
if exist "%UF2CONV%" goto :uf2_run
echo   NOTE: UF2CONV points at a file that does not exist, trying another route.
goto :uf2_cargo

:uf2_run
python "%UF2CONV%" "%PROD%.hex" -c -f %FAMNUM% -o "%PROD%.uf2" || exit /b 1
exit /b 0

:uf2_cargo
where cargo-hex-to-uf2 >nul 2>nul
if errorlevel 1 goto :uf2_skip
cargo hex-to-uf2 --input-path "%PROD%.hex" --output-path "%PROD%.uf2" --family %FAM% || exit /b 1
exit /b 0

:uf2_skip
echo   NOTE: no .uf2 written for %PROD% - set UF2CONV to a uf2conv.py, install
echo         cargo-hex-to-uf2, or use tools\verify_variant.py's .hex only. The
echo         board needs a .uf2 to be flashed by dragging, so this image is not
echo         yet flashable that way.
exit /b 0
