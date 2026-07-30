import { createHash } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';

const movie = (id, canonical, aliases) => ({ id, kind: 'movie', canonical, aliases });
const game = (id, canonical, aliases = []) => ({ id, kind: 'game', canonical, aliases });

const titles = [
  movie('spirited-away', 'Spirited Away', ['Le Voyage de Chihiro']),
  movie('lotr-fellowship', 'The Lord of the Rings: The Fellowship of the Ring', ['Le Seigneur des anneaux : La Communauté de l’anneau', 'La Communauté de l’anneau', 'LOTR 1']),
  movie('matrix', 'The Matrix', ['Matrix']),
  movie('back-to-the-future', 'Back to the Future', ['Retour vers le futur']),
  movie('die-hard', 'Die Hard', ['Piège de cristal']),
  movie('the-hangover', 'The Hangover', ['Very Bad Trip']),
  movie('jaws', 'Jaws', ['Les Dents de la mer']),
  movie('inside-out', 'Inside Out', ['Vice-Versa', 'Vice Versa']),
  movie('home-alone', 'Home Alone', ['Maman, j’ai raté l’avion !']),
  movie('groundhog-day', 'Groundhog Day', ['Un jour sans fin']),
  movie('shawshank', 'The Shawshank Redemption', ['Les Évadés']),
  movie('frozen', 'Frozen', ['La Reine des neiges']),
  movie('sound-of-music', 'The Sound of Music', ['La Mélodie du bonheur']),
  movie('raiders', 'Raiders of the Lost Ark', ['Les Aventuriers de l’arche perdue']),
  movie('star-wars-4', 'Star Wars: Episode IV – A New Hope', ['La Guerre des étoiles', 'Star Wars 4', 'SW4']),
  movie('dark-knight', 'The Dark Knight', ['The Dark Knight : Le Chevalier noir']),
  movie('good-morning-england', 'The Boat That Rocked', ['Good Morning England']),
  movie('eternal-sunshine', 'Eternal Sunshine of the Spotless Mind', ['Eternal Sunshine']),
  movie('alien', 'Alien', ['Alien, le huitième passager']),
  movie('avengers', 'The Avengers', ['Avengers']),
  game('portal', 'Portal'), game('portal-2', 'Portal 2', ['Portal II']),
  game('half-life', 'Half-Life', ['Half Life']), game('half-life-2', 'Half-Life 2', ['HL2']),
  game('witcher-3', 'The Witcher 3: Wild Hunt', ['Witcher 3', 'The Witcher III']),
  game('baldurs-gate-3', 'Baldur’s Gate 3', ['BG3', 'Baldurs Gate III']),
  game('elden-ring', 'Elden Ring'), game('hollow-knight', 'Hollow Knight', ['Hollow Nite']),
  game('celeste', 'Celeste'), game('hades', 'Hades'), game('stardew', 'Stardew Valley'),
  game('minecraft', 'Minecraft'), game('terraria', 'Terraria'),
  game('cyberpunk-2077', 'Cyberpunk 2077', ['Cyberpunk']),
  game('skyrim', 'The Elder Scrolls V: Skyrim', ['Skyrim', 'TES V']),
  game('fallout-new-vegas', 'Fallout: New Vegas', ['Fallout New Vegas', 'FNV']),
  game('mass-effect-2', 'Mass Effect 2', ['Mass Effect II', 'ME2']),
  game('bioshock', 'BioShock'), game('dishonored', 'Dishonored'),
  game('doom-eternal', 'Doom Eternal'), game('halo-ce', 'Halo: Combat Evolved', ['Halo CE']),
  game('dark-souls-3', 'Dark Souls III', ['Dark Souls 3', 'DS3']),
  game('sekiro', 'Sekiro: Shadows Die Twice', ['Sekiro']), game('control', 'Control'),
  game('alan-wake-2', 'Alan Wake 2', ['Alan Wake II']), game('dead-space', 'Dead Space'),
  game('resident-evil-4', 'Resident Evil 4', ['RE4']), game('silent-hill-2', 'Silent Hill 2', ['SH2']),
  game('mgs-v', 'Metal Gear Solid V: The Phantom Pain', ['MGS V', 'MGS5']),
  game('death-stranding', 'Death Stranding'),
  game('rdr-2', 'Red Dead Redemption 2', ['RDR2']),
  game('gta-v', 'Grand Theft Auto V', ['GTA V', 'GTA 5', 'GTAV']),
  game('assassins-creed-2', 'Assassin’s Creed II', ['Assassins Creed 2', 'AC2']),
  game('prince-persia-sot', 'Prince of Persia: The Sands of Time', ['Prince of Persia Sands of Time']),
  game('tomb-raider', 'Tomb Raider'), game('uncharted-4', 'Uncharted 4: A Thief’s End', ['Uncharted 4']),
  game('god-of-war-ragnarok', 'God of War Ragnarök', ['God of War Ragnarok', 'GOW Ragnarok']),
  game('horizon-zero-dawn', 'Horizon Zero Dawn', ['HZD']),
  game('final-fantasy-7', 'Final Fantasy VII', ['Final Fantasy 7', 'FF7']),
  game('persona-5-royal', 'Persona 5 Royal', ['P5R']), game('nier-automata', 'NieR: Automata', ['Nier Automata']),
  game('chrono-trigger', 'Chrono Trigger'),
  game('breath-wild', 'The Legend of Zelda: Breath of the Wild', ['Breath of the Wild', 'BOTW']),
  game('tears-kingdom', 'The Legend of Zelda: Tears of the Kingdom', ['Tears of the Kingdom', 'TOTK']),
  game('mario-odyssey', 'Super Mario Odyssey', ['Mario Odyssey']), game('sonic-mania', 'Sonic Mania'),
  game('metroid-dread', 'Metroid Dread'), game('pokemon-red', 'Pokémon Red', ['Pokemon Red']),
  game('civilization-6', 'Sid Meier’s Civilization VI', ['Civilization 6', 'Civ 6']),
  game('age-empires-2', 'Age of Empires II', ['Age of Empires 2', 'AOE2']),
  game('starcraft-2', 'StarCraft II', ['Starcraft 2', 'SC2']), game('world-warcraft', 'World of Warcraft', ['WoW']),
  game('diablo-2', 'Diablo II', ['Diablo 2', 'D2']), game('path-exile', 'Path of Exile', ['POE']),
  game('slay-spire', 'Slay the Spire'), game('dead-cells', 'Dead Cells'), game('cuphead', 'Cuphead'),
  game('outer-wilds', 'Outer Wilds'), game('subnautica', 'Subnautica'), game('factorio', 'Factorio'),
  game('satisfactory', 'Satisfactory'), game('rimworld', 'RimWorld', ['Rim World']),
  game('disco-elysium', 'Disco Elysium'), game('undertale', 'Undertale')
];

const cases = [
  ['spirited-away', 'le voyage de chihiro', 'accepted'],
  ['lotr-fellowship', 'la communaute de lanneau', 'accepted'],
  ['back-to-the-future', 'retour vers le futur', 'accepted'],
  ['die-hard', 'piege de cristal', 'accepted'],
  ['home-alone', 'maman jai rate lavion', 'accepted'],
  ['star-wars-4', 'sw4', 'accepted'],
  ['baldurs-gate-3', 'bg3', 'accepted'],
  ['witcher-3', 'witchr 3', 'accepted'],
  ['elden-ring', 'eldern ring', 'accepted'],
  ['hollow-knight', 'hollow nite', 'accepted'],
  ['cyberpunk-2077', 'cyberpunk2077', 'accepted'],
  ['skyrim', 'skirym', 'accepted'],
  ['fallout-new-vegas', 'fallout new vgas', 'accepted'],
  ['mass-effect-2', 'mass efect 2', 'accepted'],
  ['dark-souls-3', 'dark souls 3', 'accepted'],
  ['mgs-v', 'mgs5', 'accepted'],
  ['gta-v', 'gta5', 'accepted'],
  ['god-of-war-ragnarok', 'god of war ragnarok', 'accepted'],
  ['final-fantasy-7', 'ff7', 'accepted'],
  ['breath-wild', 'botw', 'accepted'],
  ['tears-kingdom', 'totk', 'accepted'],
  ['civilization-6', 'civ6', 'accepted'],
  ['age-empires-2', 'aoe2', 'accepted'],
  ['world-warcraft', 'wow', 'accepted'],
  ['elden-ring', 'elden kings', 'abstained'],
  ['elden-ring', 'dark souls', 'rejected'],
  ['portal', 'portal 2', 'rejected'],
  ['hades', 'had', 'rejected']
].map(([targetId, input, expected]) => ({ targetId, input, expected }));

const titleDocument = JSON.stringify({ version: 1, titles }, null, 2) + '\n';
const caseDocument = JSON.stringify({ version: 1, cases }, null, 2) + '\n';
const titleHash = createHash('sha256').update(titleDocument).digest('hex');
const packageRoot = new URL('../packages/starter-titles/', import.meta.url);
const packageData = new URL('data/', packageRoot);
const packageProfile = new URL('profile/', packageRoot);

await mkdir(new URL('../tests/corpus/', import.meta.url), { recursive: true });
await mkdir(packageData, { recursive: true });
await mkdir(packageProfile, { recursive: true });
await writeFile(new URL('../tests/corpus/titles.json', import.meta.url), titleDocument);
await writeFile(new URL('../tests/corpus/cases.json', import.meta.url), caseDocument);
await writeFile(new URL('titles.json', packageData), titleDocument);
await writeFile(new URL('context-package.schema.json', packageProfile), await readFile(new URL('../contracts/context-package.schema.json', import.meta.url)));
await writeFile(new URL('title-resource.schema.json', packageProfile), await readFile(new URL('../contracts/title-resource.schema.json', import.meta.url)));

const descriptor = {
  $schema: 'profile/context-package.schema.json',
  name: 'semantic-engine-starter-titles',
  id: 'urn:semantic-engine:context:starter-titles',
  title: 'Semantic Engine Starter Titles',
  description: 'Curated movie and video-game targets for demos and contract tests.',
  version: '0.1.0',
  created: '2026-07-30T00:00:00Z',
  licenses: [{
    name: 'CC0-1.0',
    path: 'https://creativecommons.org/publicdomain/zero/1.0/',
    title: 'Creative Commons Zero v1.0 Universal',
  }],
  contributors: [{ title: 'Semantic Engine contributors', roles: ['creator', 'dataCurator'] }],
  sources: [{ title: 'Manually curated Semantic Engine test corpus', version: '0.1.0' }],
  semanticEngine: {
    formatVersion: '0.1.0',
    kind: 'recognition-context',
    locales: ['en', 'fr'],
    spdxLicenseExpression: 'CC0-1.0',
  },
  resources: [{
    name: 'titles',
    path: 'data/titles.json',
    format: 'json',
    mediatype: 'application/json',
    encoding: 'utf-8',
    bytes: Buffer.byteLength(titleDocument),
    hash: `sha256:${titleHash}`,
    semanticEngine: { role: 'targets', schema: 'profile/title-resource.schema.json' },
  }],
};
await writeFile(new URL('datapackage.json', packageRoot), JSON.stringify(descriptor, null, 2) + '\n');
console.log(`wrote ${titles.length} titles (${titles.filter((x) => x.kind === 'movie').length} movies, ${titles.filter((x) => x.kind === 'game').length} games), ${cases.length} cases, and starter Data Package sha256:${titleHash}`);
