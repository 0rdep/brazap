# ggml drops the `lib` prefix from its static libraries on every Windows
# toolchain. Rust's windows-gnu linker searches for `libggml.a`, so restore the
# MinGW naming once transcribe.cpp has defined its targets. Loaded through
# CMAKE_PROJECT_INCLUDE (passed in CMAKE_ARGS) by scripts/build-windows; MSVC builds don't need it.
if(NOT COMMAND brazap_mingw_ggml_prefix)
  function(brazap_mingw_ggml_prefix)
    foreach(target ggml ggml-base ggml-cpu ggml-blas)
      if(TARGET ${target})
        set_target_properties(${target} PROPERTIES PREFIX "lib")
      endif()
    endforeach()
  endfunction()
  cmake_language(DEFER DIRECTORY "${CMAKE_SOURCE_DIR}" CALL brazap_mingw_ggml_prefix)
endif()
