! Reference golden-vector generator for the MSK144 port.
!
! Links the UNMODIFIED wsjtx routines genmsk_128_90.f90 (tone mapping),
! encode_128_90.f90 (CRC-13 + LDPC(128,90) systematic encode), and the real
! boost-backed crc13.cpp. A controlled 77-bit message is injected through a
! packjt77 stub (see stub_packjt77_inject.f90), so the golden vector exercises
! the reference CRC/LDPC/MSK stages bit-exactly without the full WSJT-X packer
! (whose output is produced bit-exactly by omnimodem's own message77::pack77,
! validated elsewhere).
!
! Build/run: see scratch/refvectors/msk144/build_msk144.sh
program msk144_dump
  use, intrinsic :: iso_c_binding, only: c_loc
  use crc
  use packjt77, only: inject_c77
  implicit none

  character(len=200) :: arg
  character*37 :: msg0, msgsent
  character*90 :: tmpchar
  integer*4 :: i4tone(144)
  integer*1 :: msgbits(77), codeword(128)
  integer*1, target :: i1MsgBytes(12)
  integer :: i, ichk, itype, ncrc13
  character*77 :: pat

  ! Default synthetic 77-bit pattern: bit=1 where (i-1) mod 3 == 0.
  do i = 1, 77
     if (mod(i-1,3) == 0) then
        pat(i:i) = '1'
     else
        pat(i:i) = '0'
     endif
  end do
  ! Optional CLI override: a 77-char '0'/'1' string.
  if (command_argument_count() >= 1) then
     call get_command_argument(1, arg)
     if (len_trim(arg) == 77) pat = arg(1:77)
  endif

  inject_c77 = pat
  read(pat,'(77i1)') msgbits

  ! --- Tones via the unmodified reference tone generator ---
  msg0 = 'INJECTED'
  ichk = 0
  call genmsk_128_90(msg0, ichk, msgsent, i4tone, itype)

  ! --- Codeword via the unmodified reference encoder ---
  call encode_128_90(msgbits, codeword)

  ! --- CRC-13 via the real boost crc13 (same byte layout as encode_128_90) ---
  write(tmpchar,'(77i1)') msgbits
  tmpchar(78:80) = '000'
  i1MsgBytes = 0
  read(tmpchar,'(10b8)') i1MsgBytes(1:10)
  ncrc13 = crc13(c_loc(i1MsgBytes), 12)

  write(*,'(A)') 'msg77'
  write(*,'(77i1)') msgbits
  write(*,'(A)') 'crc13'
  write(*,'(b13.13)') ncrc13
  write(*,'(A)') 'codeword128'
  write(*,'(128i1)') codeword
  write(*,'(A)') 'tones144'
  write(*,'(144i1)') i4tone(1:144)
end program msk144_dump
