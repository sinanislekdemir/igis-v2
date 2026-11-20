unit clock;

uses core, syslib;

// Read the CMOS real-time clock through x86 port I/O. This is the first
// application that drives a device through `syslib`: register A bit 7 is the
// update-in-progress flag, and registers 0/2/4 hold seconds, minutes and hours
// as BCD. A bounded poll keeps a broken RTC from hanging the task.
//
// Limits of this slice: the registers are assumed to be BCD (QEMU's default),
// there is no status-B inquiry, and the hour's 12/24 flag (bit 7) is dropped
// rather than converted, so a 12-hour RTC prints 01..12.
var
  hour: string[2];
  minute: string[2];
  second: string[2];
  status: uint;
  guard: uint;
  seconds: uint;
  minutes: uint;
  hours: uint;

procedure select_register(reg: uint);
begin
  syslib.port_out8(uint(0x70), reg);
end;

procedure read_register(reg: uint; value: var uint);
begin
  select_register(reg);
  value := syslib.port_in8(uint(0x71));
end;

procedure print_two(value: uint; dst: var string[2]);
var
  written: int;
begin
  written := core.format_int_into(dst, int(value), uint(10), uint(2));
end;

procedure main(args: const string[1024]);
begin
  // Wait until register A reports that no update is in progress (bit 7 clear;
  // register A has no other bit above 0x7F, so the test is `value >= 0x80`).
  guard := uint(0);
  syslib.port_out8(uint(0x70), uint(0x0A));
  status := syslib.port_in8(uint(0x71));
  while status >= uint(0x80) and guard < uint(1000000) do
  begin
    syslib.port_out8(uint(0x70), uint(0x0A));
    status := syslib.port_in8(uint(0x71));
    guard := guard + uint(1);
  end;

  read_register(uint(0), seconds);
  read_register(uint(2), minutes);
  read_register(uint(4), hours);

  // BCD to binary: (v div 16) * 10 + (v mod 16). Drop the hour's 12/24 flag
  // first; in 24-hour mode it is already zero for 00..23.
  hours := hours mod uint(0x80);
  seconds := (seconds div uint(16)) * uint(10) + (seconds mod uint(16));
  minutes := (minutes div uint(16)) * uint(10) + (minutes mod uint(16));
  hours := (hours div uint(16)) * uint(10) + (hours mod uint(16));

  // Width 2 zero-pads each field, so the output is HH:MM:SS.
  print_two(hours, hour);
  print_two(minutes, minute);
  print_two(seconds, second);
  core.write(hour);
  core.write(":");
  core.write(minute);
  core.write(":");
  core.write(second);
  core.write("\n");
end;
