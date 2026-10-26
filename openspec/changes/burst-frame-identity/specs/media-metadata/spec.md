# Spec Delta

## ADDED Requirements

### Requirement: Frame identity
The engine SHALL read a frame identity where the file records one: a DNG's OriginalRawFileName stem, and the Canon maker-note file number, also from maker notes kept in DNGPrivateData. It SHALL read no bytes beyond those the probe already reads. Maker notes in DNGPrivateData SHALL count as maker notes.

#### Scenario: Converted DNG records its frame
- **WHEN** a DNG converted from IMG_0034.CR2, with a Canon maker note in DNGPrivateData giving file number 101-0034, is analysed
- **THEN** its metadata has the OriginalRawFileName frame IMG_0034, the Canon file number 1010034, and maker notes present

#### Scenario: Frame identity costs no extra read
- **WHEN** a 25 MB converted DNG is analysed through a byte-counting file layer
- **THEN** the bytes read are the same as before frame identity was read
