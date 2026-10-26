# Third-party notices

SerialLoom includes the following third-party software.

## usbip-win2

SerialLoom bundles unmodified, Microsoft-signed files from [usbip-win2](https://github.com/vadimgrn/usbip-win2) (the virtual USB host controller drivers `usbip2_ude` and `usbip2_filter`). SerialLoom uses them to plug its software USB serial devices into Windows, talking to the controller through its documented driver interface (`include/usbip/vhci.h`). usbip-win2 does not endorse SerialLoom.

```
BSD 2-Clause License

Copyright (c) 2021-2026, Vadym Hrynchyshyn <vadimgrn@gmail.com>

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

The authoritative license text ships as `usbip\LICENSE.txt` next to SerialLoom and is copied from the usbip-win2 release by `scripts/fetch-usbip-win2.ps1`.

## Windows USB serial driver

The COM ports themselves are created by Windows' built-in `usbser.sys` driver, which is part of Windows and is not distributed with SerialLoom.
