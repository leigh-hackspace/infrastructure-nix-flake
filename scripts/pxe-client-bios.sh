#!/usr/bin/env bash

sudo qemu-system-x86_64 \
  -m 8192 \
  -accel kvm \
  -smp 4 \
  -netdev tap,id=net0,br=br227,helper=$(type -p qemu-bridge-helper) \
  -device virtio-net-pci,netdev=net0 \
  -display vnc=:0 \
  -vga qxl \
  -boot n \
