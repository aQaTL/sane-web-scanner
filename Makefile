SHELL := /bin/bash

default:
	cargo check

run: dev

dev build start generate lint:
	cd frontend && npm run $@