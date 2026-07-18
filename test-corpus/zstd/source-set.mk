.PHONY: ccc-zstd-source-set
ccc-zstd-source-set:
	@printf '%s\n' $(ZSTDLIB_FULL_SRC) $(addprefix $(CURDIR)/,$(ZSTD_CLI_SRC))
