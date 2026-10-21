// Which rows of the parameter guide describe which patch/system settings: { guide page title:
// [[row name in the guide ('' = the page's own text, or its unnamed table), parameter path(s)]] }.
// Paths use `(n)` for instance numbers. Effects are matched automatically by name (manual.mjs).

const controls = ['Num1', 'Num2', 'Num3', 'Num4', 'CNum', 'BankDown', 'BankUp', 'Ctl1', 'Ctl2', 'Ctl3', 'Ctl4',
  'Exp1Sw', 'Exp1', 'Exp2', 'ManualNum1', 'ManualNum2', 'ManualNum3', 'ManualNum4'];
const both = (suffix) => controls.flatMap((c) => [`Temp_patch.common.${c}${suffix}`, `System_control.${c}${suffix}`])
  .filter((p) => !/Exp[12]Mode$/.test(p));
const pmidi = (field) => [1, 2, 3, 4].map((i) => `Temp_patch.common.PMIDI${i}_${field}`);
const assign = (field) => `Temp_patch.assign(n).${field}`;

export const SETTINGS = {
  'MASTER': [
    ['MEMORY LEVEL', 'Temp_patch.efct.MasterPatchLevel'],
    ['BPM', 'Temp_patch.efct.MasterBpm'],
    ['KEY', 'Temp_patch.efct.MasterKey'],
    ['AMP CTL1, AMP CTL2', ['Temp_patch.efct.MasterAmpCtl1', 'Temp_patch.efct.MasterAmpCtl2']],
    ['CARRYOVER', 'Temp_patch.efct.MasterCarryOver'],
    ['TEMPO HOLD', 'Temp_patch.efct.MasterTempoHold'],
  ],
  'CONTROL FUNCTION': [['', both('Function')]],
  'ASSIGN SETTING': [
    ['SW', assign('SW')],
    ['TARGET MIN', assign('TARGET_MIN')],
    ['TARGET MAX', assign('TARGET_MAX')],
    ['SOURCE', assign('SOURCE')],
    ['SOURCE MODE', [assign('MODE'), ...both('Mode')]],
    ['SOURCE ACT LOW', assign('ACT_RANGE_LO')],
    ['SOURCE ACT HIGH', assign('ACT_RANGE_HI')],
    ['SOURCE SENS', 'Temp_patch.efct.MasterInputSens'],
    ['INTERNAL PEDAL TRIGGER *1', assign('INT_PDL_TRIGGER')],
    ['INTERNAL PEDAL TIME *1', assign('INT_PDL_TIME')],
    ['INTERNAL PEDAL CURVE *1', assign('INT_PDL_CURVE')],
    ['WAVE PEDAL FORM *2', assign('WAVE_FORM')],
    ['WAVE PEDAL RATE *2', assign('WAVE_RATE')],
    ['MIDI CH *3 *4', assign('MIDI_CH')],
    ['TARGET MIDI CC# *3 CC#', assign('MIDI_CC_NO')],
    ['TARGET MIDI CC# *3 MIN', assign('MIDI_CC_VALUE_MIN')],
    ['TARGET MIDI CC# *3 MAX', assign('MIDI_CC_VALUE_MAX')],
    ['TARGET MIDI PC# *4 PC#', assign('MIDI_PC_NO')],
    ['TARGET MIDI PC# *4 MSB', assign('MIDI_BANK_MSB')],
    ['TARGET MIDI PC# *4 LSB', assign('MIDI_BANK_LSB')],
  ],
  'MEMORY MIDI': [
    ['CH', pmidi('CH')],
    ['BANK MSB\nBANK LSB', [...pmidi('BANK_MSB'), ...pmidi('BANK_LSB')]],
    ['PC#', pmidi('PC_NO')],
    ['CC1#\nCC2#', [...pmidi('CC1_NO'), ...pmidi('CC2_NO')]],
    ['CC1 VALUE\nCC2 VALUE', [...pmidi('CC1_VALUE'), ...pmidi('CC2_VALUE')]],
  ],
  'CONTROL MODE': [['', 'System_control.ControlMode']],
  'AMP CONTROL': [['AMP CTL1\nAMP CTL2', ['System_common.AmpCtl1', 'System_common.AmpCtl2']]],
  'EXP HOLD': [['EXP1 HOLD\nEXP2 HOLD', ['System_common.Exp1Hold', 'System_common.Exp2Hold']]],
  'COLOR MODE': [['COLOR MODE', 'System_common.ColorMode']],
  'AUTO OFF': [['AUTO OFF', 'System_common.AutoOff']],
  'PLAY OPTION': [
    ['BANK MODE', 'System_common.BankChangeMode'],
    ['BANK EXTENT MIN', 'System_common.BankExtentMin'],
    ['BANK EXTENT MAX', 'System_common.BankExtentMax'],
    ['LOOP MODE', 'System_efct.LooperMode'],
    ['LOOP REC ACTION', 'System_efct.LooperRecAction'],
    ['DELETE WARNING', 'System_common.WarningDelete'],
    ['OVERWRITE WARNING', 'System_common.WarningOverwrite'],
  ],
  'TUNER': [
    ['[1] REF. PITCH', 'System_pitch.ReferencePitch'],
    ['[2] TUNER OUTPUT', 'System_pitch.TunerOutput'],
    ['[3] TT TYPE', 'System_pitch.PolyTunerType'],
    ['[4] TT OFFSET', 'System_pitch.PolyTunerOffset'],
  ],
  'MIDI SETTING': [
    ['RX CHANNEL', 'System_midi.RxChannel'],
    ['TX CHANNEL', 'System_midi.TxChannel'],
    ['MIDI IN THRU', 'System_midi.MidiInThru'],
    ['USB IN THRU', 'System_midi.UsbInThru'],
    ['SYNC CLOCK', 'System_midi.SyncClock'],
    ['CLOCK OUT', 'System_midi.ClockOut'],
    ['NUM1 CC#\nNUM2 CC#\nNUM3 CC#\nNUM4 CC#\nBANK ▲ CC#\nBANK ▼ CC#\nEXP1 SW CC#\nCTL1 CC#\nCTL2 CC#\nCTL3 CC#\nCTL4 CC#\nEXP1 CC#\nEXP2 CC#',
      ['Num1', 'Num2', 'Num3', 'Num4', 'Bankup', 'Bankdown', 'Exp1Sw', 'Ctl1', 'Ctl2', 'Ctl3', 'Ctl4', 'Exp1', 'Exp2']
        .map((c) => `System_midi.${c}Cc`)],
  ],
  'PROGRAM MAP BANK1–BANK3': [['MAP SELECT', 'System_midi.MapSelect']],
  'USB': [
    ['EFX OUT LEVEL', 'System_inout.UsbAudioMainEfxOutLevel'],
    ['MIX LEVEL', 'System_inout.UsbAudioMainMixLevel'],
    ['DRY OUT', 'System_inout.UsbAudioDryOutLevel'],
    ['DRY TO EFX', 'System_inout.UsbAudioDryToEfxLevel'],
    ['DIRECT MONITOR', 'Setup_efct.UsbAudioDirMonitorSw'],
    ['LOOPBACK', 'System_inout.UsbAudioLoopBack'],
  ],
  'INPUT SETTINGS': [
    ['NUM', 'System_control.InputSettingNum'],
    ['INPUT', 'Setup_efct.IOInstType'],
    ['INPUT SENS', 'Setup_efct.IOInputLevel'],
  ],
  'OUTPUT SELECT': [['', 'System_inout.OutputSelect']],
  'GLOBAL EQ': [
    ['ON/OFF', 'System_control.GlobalEqSw'],
    ['LOW GAIN', 'Setup_efct.IOOutGlobalEqLowGain'],
    ['HIGH GAIN', 'Setup_efct.IOOutGlobalEqHighGain'],
    ['LEVEL', 'Setup_efct.IOOutGlobalEqLevel'],
    ['LOW-MID FREQ', 'Setup_efct.IOOutGlobalEqLowMidFreq'],
    ['LOW-MID Q', 'Setup_efct.IOOutGlobalEqLowMidQ'],
    ['LOW-MID GAIN', 'Setup_efct.IOOutGlobalEqLowMidGain'],
    ['HIGH-MID FREQ', 'Setup_efct.IOOutGlobalEqHighMidFreq'],
    ['HIGH-MID Q', 'Setup_efct.IOOutGlobalEqHighMidQ'],
    ['HIGH-MID GAIN', 'Setup_efct.IOOutGlobalEqHighMidGain'],
    ['LOW CUT', 'Setup_efct.IOOutGlobalEqLowCut'],
    ['HIGH CUT', 'Setup_efct.IOOutGlobalEqHighCut'],
  ],
  'OUTPUT SETTING': [['', 'System_inout.TotalMainOutputLevel']],
};
