SHELL := /bin/bash

default:
	cargo check

run export:
	$(MAKE) -C frontend $@