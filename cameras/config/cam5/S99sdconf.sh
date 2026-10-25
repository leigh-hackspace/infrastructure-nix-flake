#!/bin/sh
if [ -f /mnt/mmcblk0p1/sdconf.sh ]; then
    echo "======== SD CONFIG DETECTED ========"
    /bin/sh /mnt/mmcblk0p1/sdconf.sh
else
    echo "======== SD CONFIG NOT DETECTED ========"
fi
