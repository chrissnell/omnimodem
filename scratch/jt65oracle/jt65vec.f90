program jt65vec
  use packjt
  use iso_c_binding
  include 'testmsg.f90'
  interface
     subroutine gen65(msg00,ichk,msgsent0,itone,itype) bind(c)
       import :: c_char, c_int
       character(kind=c_char) :: msg00(23), msgsent0(23)
       integer(c_int) :: ichk, itone(126), itype
     end subroutine gen65
  end interface
  character*22 msg
  character(kind=c_char) msg00(23), msgsent0(23)
  integer dgen(13),sent(63),itype,ichk,i,j,iz
  integer(c_int) itone(126)
  do i=1,NTEST
     msg=testmsg(i)
     call fmtmsg(msg,iz)
     call packmsg(msg,dgen,itype)
     call rs_encode(dgen,sent)
     do j=1,22
        msg00(j)=msg(j:j)
     enddo
     msg00(23)=char(0)
     ichk=0
     call gen65(msg00,ichk,msgsent0,itone,itype)
     write(*,'(A)',advance='no') 'MSG|'
     write(*,'(A)',advance='no') trim(msg)
     write(*,'(A)',advance='no') '|itype|'
     write(*,'(I0)',advance='no') itype
     write(*,'(A)',advance='no') '|dgen|'
     do j=1,12
        write(*,'(I0,1X)',advance='no') dgen(j)
     enddo
     write(*,'(A)',advance='no') '|sent|'
     do j=1,63
        write(*,'(I0,1X)',advance='no') sent(j)
     enddo
     write(*,'(A)',advance='no') '|itone|'
     do j=1,126
        write(*,'(I0,1X)',advance='no') itone(j)
     enddo
     write(*,*)
  enddo
end program jt65vec
