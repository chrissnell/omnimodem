! Minimal stub of the packjt77 module so genfst4.f90 links for the
! get_fst4_tones_from_bits entry (which never calls pack77/unpack77 — the
! message-packing path is bypassed when we supply msgbits directly).
module packjt77
contains
  subroutine pack77(msg0,i3,n3,c77)
    character*37 msg0; integer i3,n3; character*77 c77; c77=repeat('0',77)
  end subroutine
  subroutine unpack77(c77,nrx,msg,unpk77_success)
    character*77 c77; integer nrx; character*37 msg; logical unpk77_success
    msg=' '; unpk77_success=.true.
  end subroutine
end module packjt77
