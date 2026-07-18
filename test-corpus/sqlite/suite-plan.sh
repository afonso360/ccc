#!/usr/bin/env bash

select_sqlite_suite() {
  case "$1" in
    veryquick)
      suite_driver=make-target
      suite_make_target=tcltest
      suite_tcl_entrypoint=test/veryquick.test
      suite_components=tcltest
      suite_command='make -j1 tcltest'
      ;;
    quick)
      # SQLite's `quicktest` Make target runs extraquick.test, which is smaller
      # than veryquick.test. Invoke the upstream quick.test entrypoint directly
      # so this mode retains SQLite's own meaning of "quick".
      suite_driver=test-script
      suite_make_target=testfixture
      suite_tcl_entrypoint=test/quick.test
      suite_components=quick
      suite_command='make -j1 testfixture && ./testfixture "$TOP/test/quick.test" --verbose=file --output=test-out.txt'
      ;;
    all)
      suite_driver=make-target
      suite_make_target=alltest
      suite_tcl_entrypoint=test/all.test
      suite_components=alltest
      suite_command='make -j1 alltest'
      ;;
    full)
      suite_driver=make-target
      suite_make_target=fulltest
      suite_tcl_entrypoint=test/all.test
      suite_components=alltest,fuzztest
      suite_command='make -j1 fulltest'
      ;;
    *)
      echo "unsupported SQLite test suite: $1" >&2
      return 2
      ;;
  esac
}

run_sqlite_suite() {
  local source_directory=$1
  local build_compiler=$2
  local target_compiler=$3

  if [[ "$suite_driver" == make-target ]]; then
    make -j1 BCC="$build_compiler -g" CC="$target_compiler" \
      "$suite_make_target"
  else
    make -j1 BCC="$build_compiler -g" CC="$target_compiler" \
      "$suite_make_target"
    ./testfixture "$source_directory/$suite_tcl_entrypoint" \
      --verbose=file --output=test-out.txt
  fi
}
