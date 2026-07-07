! Minimal stub of the packjt77 module used to feed a *controlled* 77-bit
! message into the UNMODIFIED genmsk_128_90.f90 tone generator, so the golden
! channel-symbol vector exercises the reference CRC-13 + LDPC(128,90) + MSK
! tone mapping without linking the full WSJT-X message packer. The 77-bit
! payload is injected via the module variable `inject_c77` (a '0'/'1' string).
module packjt77
  character*77 :: inject_c77 = repeat('0',77)
contains
  subroutine pack77(msg0,i3,n3,c77)
    character*37 msg0; integer i3,n3; character*77 c77
    i3=1; n3=-1
    c77=inject_c77
  end subroutine
  subroutine unpack77(c77,nrx,msg,unpk77_success)
    character*77 c77; integer nrx; character*37 msg; logical unpk77_success
    msg=' '; unpk77_success=.true.
  end subroutine
end module packjt77
