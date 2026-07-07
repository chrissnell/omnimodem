! Authoritative JS8 tone-assembly dump. Replicates genjs8.f90's Costas insertion
! + data->tone mapping (S7 D29 S7 D29 S7, plain-binary 3-bit map) for a fixed
! 87-bit message, for both Costas variants (NCOSTAS=1 original / =2 symmetrical).
! Pure Fortran (reference encode174; no CRC). ref: js8call/lib/js8/genjs8.f90.
program dump_itone
  implicit none
  integer, parameter :: N=174, K=87, ND=58, NS=21, NN=79
  integer*1 :: message(K), codeword(N)
  integer :: itone(NN), icos7a(0:6), icos7b(0:6), icos7c(0:6)
  integer :: i, j, k2, indx, icos
  character(len=NN*3) :: s

  do i=1,K
    message(i) = iand( ishft(i*37+5, -1), 1 )
  enddo
  call encode174(message, codeword)

  do icos=1,2
    if(icos.eq.1) then
      icos7a = (/4,2,5,6,1,3,0/); icos7b = (/4,2,5,6,1,3,0/); icos7c = (/4,2,5,6,1,3,0/)
    else
      icos7a = (/0,6,2,3,5,4,1/); icos7b = (/1,5,0,2,3,6,4/); icos7c = (/2,5,0,6,4,1,3/)
    endif
    itone = -1
    itone(1:7)=icos7a
    itone(36+1:36+7)=icos7b
    itone(NN-6:NN)=icos7c
    k2=7
    do j=1,ND
      i=3*j-2
      k2=k2+1
      if(j.eq.30) k2=k2+7
      indx=codeword(i)*4 + codeword(i+1)*2 + codeword(i+2)
      itone(k2)=indx
    enddo
    s=''
    do i=1,NN
      write(s((i-1)*3+1:(i-1)*3+3),'(I2,1X)') itone(i)
    enddo
    if(icos.eq.1) then
      write(*,'(A)') 'ITONE_ORIG '//trim(adjustl(s))
    else
      write(*,'(A)') 'ITONE_SYM '//trim(adjustl(s))
    endif
  enddo
end program
