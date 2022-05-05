# Change Log

## [2.2.0] - 2022-05-04
- The aMOF criterion was isol-01 being above 90%. This turned out to be too
  strict for many devices leading to overly pessimistic solutions or no
  solutions on lower end devices. Another problem is that it made the
  benchmark too sensitive to disturbances which may not be coming from the
  IO device. Relax the criterion to isol-05, which still provides a pretty
  strong guarantee while making the benchmark more robust.
- aMOF is one of the most important metrics that iocost-tune uses in
  determining the parameter solutions. However, because of the limited line
  fitting that it could do, it wasn't well utilized - e.g. some, usually
  high performing, devices show aMOF data points which peak in the middle,
  which fits the isolated-bandwidth solution perfectly. However, due to the
  limitations in line fitting, we couldn't detect this mid peak and instead
  relied on lat-imp which is a lot less reliable. Line fitting is improved
  to address this shortcoming.

## [2.1.2] - 2021-09-10
- Fix bench merge mode rejecting sources spuriously on ignored sysreqs.
- Improve visibility on merge fails.
- Merge now only cares about the major and minor semantic versions and will
  try merging results from e.g. 2.1.0, 2.1.1 and 2.1.2 together by default.
- CHANGELOG added.

## [2.1.1] - 2021-08-23
- bench study mode fix.

## [2.1.0] - 2021-08-23
- iocost-tune solution criteria updated.

## [2.0.0] - 2021-06-24
- resctl-bench added.

## [1.0.0] - 2020-10-13
- resctl-demo initial release.
