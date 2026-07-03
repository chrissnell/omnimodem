! Minimal stub of the prog_args module so gen_fst4wave.f90 links standalone
! (gen_fst4wave only references get_environment_variable, an intrinsic).
module prog_args
  character(len=500) :: data_dir = ' '
end module prog_args
