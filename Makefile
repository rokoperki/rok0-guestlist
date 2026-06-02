SRC    := src/rok0_guestbook/rok0_guestbook.s
SO     := deploy/rok0_guestbook.so
RUNNER := agave-ledger-tool program run
LEDGER := test-ledger
MODE   := --mode interpreter
IX_DIR := src/rok0_guestbook

.PHONY: all build run-register run-heartbeat run-promote test clean

all: build test

build: $(SO)

$(SO): $(SRC)
	@echo "==> building sBPF"
	llvm-mc -arch=bpfel -filetype=obj -o /tmp/rok0_guestbook.o $(SRC)
	llvm-objcopy \
		--output-target=binary \
		--only-section=.text \
		/tmp/rok0_guestbook.o \
		/tmp/rok0_guestbook_text.bin
	llvm-objcopy \
		-I binary -O elf64-little \
		--rename-section=.data=.text \
		/tmp/rok0_guestbook_text.bin \
		$(SO)

run-register: $(SO)
	@echo "==> run register"
	$(RUNNER) $(SO) \
		--ledger $(LEDGER) \
		$(MODE) \
		--input $(IX_DIR)/instructions_register.json \
		--trace trace_register.txt

run-heartbeat: $(SO)
	@echo "==> run heartbeat"
	$(RUNNER) $(SO) \
		--ledger $(LEDGER) \
		$(MODE) \
		--input $(IX_DIR)/instructions_heartbeat.json \
		--trace trace_heartbeat.txt

run-promote: $(SO)
	@echo "==> run promote"
	$(RUNNER) $(SO) \
		--ledger $(LEDGER) \
		$(MODE) \
		--input $(IX_DIR)/instructions_promote.json \
		--trace trace_promote.txt

test: $(SO)
	@echo "==> cargo test"
	cargo test

clean:
	rm -f /tmp/rok0_guestbook.o /tmp/rok0_guestbook_text.bin trace_register.txt trace_heartbeat.txt
