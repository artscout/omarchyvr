CC ?= cc
CFLAGS ?= -O2 -g
CFLAGS += -std=c11 -Wall -Wextra -Iinclude
PREFIX ?= /usr/local
B = build

LIBSRC = src/pimax.c src/decode.c
OBJS = $(LIBSRC:src/%.c=$(B)/%.o)
SOBJS = $(LIBSRC:src/%.c=$(B)/%.pic.o)

all: $(B)/libpimax.a $(B)/libpimax.so $(B)/pimax-cli

$(B):
	mkdir -p $(B)

$(B)/%.o: src/%.c include/pimax.h | $(B)
	$(CC) $(CFLAGS) -c $< -o $@
$(B)/%.pic.o: src/%.c include/pimax.h | $(B)
	$(CC) $(CFLAGS) -fPIC -c $< -o $@

$(B)/libpimax.a: $(OBJS)
	$(AR) rcs $@ $^
$(B)/libpimax.so: $(SOBJS)
	$(CC) -shared -pthread -Wl,-soname,libpimax.so.0 -o $@ $^

$(B)/pimax-cli: tools/pimax-cli.c $(B)/libpimax.a
	$(CC) $(CFLAGS) $< $(B)/libpimax.a -pthread -o $@

install: all
	install -Dm644 include/pimax.h $(DESTDIR)$(PREFIX)/include/pimax.h
	install -Dm644 $(B)/libpimax.a $(DESTDIR)$(PREFIX)/lib/libpimax.a
	install -Dm755 $(B)/libpimax.so $(DESTDIR)$(PREFIX)/lib/libpimax.so
	install -Dm755 $(B)/pimax-cli $(DESTDIR)$(PREFIX)/bin/pimax-cli

clean:
	rm -rf $(B)

.PHONY: all install clean
